//! Per-entity-type attachment points, resolved against the live registry
//! (M72 — passenger positioning).
//!
//! 26.x does not carry a seat height in code. `Entity.getPassengerRidingPosition`
//! is
//!
//! ```text
//! this.position().add(attachments.getClamped(PASSENGER, indexOf(passenger), this.yRot))
//! ```
//!
//! and `attachments` comes from `EntityDimensions`, which the **entity type**
//! declares on its builder. So the seat is *data*, extracted by
//! `tools/gen_entity_attachments.py` into [`crate::entity_attachments_table`]
//! and resolved here to protocol ids.
//!
//! **There are two tables, not one.** `Entity.positionRider` is
//!
//! ```text
//! position = vehicle.getPassengerRidingPosition(passenger);   // vehicle's PASSENGER point
//! offset   = passenger.getVehicleAttachmentPoint(vehicle);    // rider's own VEHICLE point
//! moveFunction(passenger, position - offset);
//! ```
//!
//! — the second is keyed by the **rider's** type and rotated by the **rider's**
//! yaw. A player's is `(0, 0.6, 0)`, which is the whole reason a rider sits in
//! a saddle instead of standing on the horse's head; dropping it would raise
//! every mounted player by 0.6 blocks.
//!
//! This module answers "what points does this type declare"; the vanilla
//! arithmetic that consumes them lives in `rewo_world::riding`, because the
//! rotation goes through `Mth`'s sine table rather than platform trig.

use std::collections::{HashMap, HashSet};

use crate::entity_attachments_table as table;
use crate::entity_types::EntityTypes;

/// One type's declared points, with the two fallbacks already spelled out.
#[derive(Clone, Debug)]
pub struct TypePoints {
    /// `sized(width, …)` — read by `Spider.getVehicleAttachmentPoint`, the one
    /// override that compares the two entities' widths.
    pub width: f32,
    /// `sized(…, height)`.
    pub height: f32,
    /// Declared PASSENGER points in declaration order. **Empty** means the
    /// type declares none and `EntityAttachment.Fallback.AT_HEIGHT` applies:
    /// a single point at `(0, height, 0)`.
    pub passenger: &'static [[f64; 3]],
    /// The declared VEHICLE point, or `None` for `AT_FEET` (`Vec3.ZERO`).
    pub vehicle: Option<[f64; 3]>,
}

impl TypePoints {
    /// `EntityAttachments.getClamped(PASSENGER, index, …)` — **untransformed**.
    ///
    /// The clamp is vanilla's and it is observable: a fifth rider on the happy
    /// ghast's four seats takes the fourth, it does not fall back to the first
    /// and it does not vanish.
    pub fn passenger_point(&self, index: usize) -> [f64; 3] {
        if self.passenger.is_empty() {
            // AT_HEIGHT.
            return [0.0, self.height as f64, 0.0];
        }
        self.passenger[index.min(self.passenger.len() - 1)]
    }

    /// `EntityAttachments.get(VEHICLE, 0, …)` — **untransformed**.
    pub fn vehicle_point(&self) -> [f64; 3] {
        self.vehicle.unwrap_or([0.0, 0.0, 0.0])
    }

    /// How many seats the type declares, before the clamp. `0` means it
    /// declares none and rides on the AT_HEIGHT fallback.
    pub fn seats(&self) -> usize {
        self.passenger.len()
    }
}

/// The Java class a type's `getPassengerAttachmentPoint` /
/// `getVehicleAttachmentPoint` / `positionRider` override comes from.
///
/// This is the JVM's virtual dispatch made explicit. It is deliberately a
/// **single** answer per type, resolved most-derived-first, because that is
/// what `super` calls do: a camel is an `AbstractHorse`, and its own override
/// replaces the horse's rather than composing with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VehicleClass {
    /// No override — `Entity`/`LivingEntity`'s table lookup.
    Default,
    /// `AbstractBoat` — replaces the point entirely with
    /// `(0, rideHeight(dimensions), xOffset)`.
    Boat {
        /// `rideHeight` is `height * 0.8888889` for a raft, `height / 3` for a
        /// boat. The split follows the leaf class, **not** the chest/plain one.
        raft: bool,
        /// `getSinglePassengerXOffset()` — 0.15 on `AbstractChestBoat`, 0.0 on
        /// `AbstractBoat`. Used only when the boat carries one passenger.
        chest: bool,
    },
    /// `AbstractMinecart` — lowers the point to `Vec3.ZERO` for a villager or
    /// wandering trader, otherwise the default.
    Minecart,
    /// `AbstractCubeMob` — `(0, height - 0.015625 * size * scale, 0)`.
    CubeMob,
    /// `Strider` — the default plus a client-only walk bob.
    Strider,
    /// `Camel` — a two-seat body anchor. Declares no PASSENGER points, so the
    /// default would put a rider at the top of its bounding box.
    Camel,
    /// `AbstractHorse` (minus `Camel`, which overrides it, and `Llama`, whose
    /// override is a no-op restoring the default) — the default plus a rearing
    /// offset scaled by `standAnimO`.
    Horse,
}

/// Resolved attachment data for every registered type.
pub struct Attachments {
    by_id: HashMap<i32, TypePoints>,
    class: HashMap<i32, VehicleClass>,
    /// `Animal` descendants — read of the **rider** by the boat and camel
    /// second-seat bump.
    animal: HashSet<i32>,
    /// `Villager` ∪ `WanderingTrader` — read of the **rider** by the minecart.
    lowered_in_minecart: HashSet<i32>,
    /// `Spider` descendants — the only override of `getVehicleAttachmentPoint`.
    spider: HashSet<i32>,
    /// Vehicles whose `positionRider` copies `yBodyRot` onto a living rider.
    forces_body_yaw: HashSet<i32>,
    /// `LivingEntity` descendants — the cast that gates that copy. Duplicated
    /// from [`crate::entity_types::EntityClasses`] deliberately: the riding
    /// derivation runs inside `EntityTable`, which holds no classes, and
    /// threading a second object through every call site to answer one
    /// `instanceof` would be worse than resolving the same generated list
    /// twice.
    living: HashSet<i32>,
}

impl Attachments {
    /// Resolve the generated table against the runtime registry.
    ///
    /// Hard-fails on drift, the same three-part contract the other generated
    /// tables use: a registry of a different size than the pin, a generated
    /// name the registry does not contain, or a class set that came back
    /// empty (which would mean the class was renamed and its override had
    /// silently stopped being selected).
    pub fn resolve(types: &EntityTypes) -> Result<Self, String> {
        use crate::entity_classes as classes;
        if types.len() != table::SCANNED_TYPES {
            return Err(format!(
                "entity_attachments: the entity_type registry has {} entries but the \
                 generated table was built from {} — re-run \
                 tools/gen_entity_attachments.py after the version bump",
                types.len(),
                table::SCANNED_TYPES
            ));
        }
        let mut by_id = HashMap::new();
        for row in table::TYPES {
            let id = types.id_of(row.name).ok_or_else(|| {
                format!(
                    "entity_attachments: generated type {:?} is not in the entity_type \
                     registry — re-run tools/gen_entity_attachments.py",
                    row.name
                )
            })?;
            by_id.insert(
                id,
                TypePoints {
                    width: row.width,
                    height: row.height,
                    passenger: row.passenger,
                    vehicle: row.vehicle,
                },
            );
        }
        let set = |names: &[&str], what: &str| -> Result<HashSet<i32>, String> {
            if names.is_empty() {
                return Err(format!(
                    "entity_attachments: the {what} class set is empty — the Java class \
                     was renamed and its override would silently stop being selected"
                ));
            }
            names
                .iter()
                .map(|n| {
                    types.id_of(n).ok_or_else(|| {
                        format!(
                            "entity_attachments: generated {what} entity {n:?} is not in \
                             the entity_type registry — re-run tools/gen_entity_classes.py"
                        )
                    })
                })
                .collect()
        };
        let boats = set(classes::ABSTRACT_BOAT, "boat")?;
        let chest_boats = set(classes::ABSTRACT_CHEST_BOAT, "chest-boat")?;
        let rafts = set(classes::RAFT, "raft")?;
        let chest_rafts = set(classes::CHEST_RAFT, "chest-raft")?;
        let minecarts = set(classes::ABSTRACT_MINECART, "minecart")?;
        let cube_mobs = set(classes::ABSTRACT_CUBE_MOB, "cube-mob")?;
        let striders = set(classes::STRIDER, "strider")?;
        let camels = set(classes::CAMEL, "camel")?;
        let horses = set(classes::ABSTRACT_HORSE, "horse")?;
        let chickens = set(classes::CHICKEN, "chicken")?;
        let animal = set(classes::ANIMAL, "animal")?;
        let spider = set(classes::SPIDER, "spider")?;
        let mut lowered_in_minecart = set(classes::VILLAGER, "villager")?;
        lowered_in_minecart.extend(set(classes::WANDERING_TRADER, "wandering-trader")?);

        // `AbstractChestBoat extends AbstractBoat`, so a chest boat that is not
        // in `boats` would mean the two generated sets disagree about the
        // hierarchy — and the `rideHeight` selection below would silently pick
        // the plain-boat branch.
        if let Some(bad) = chest_boats.iter().find(|id| !boats.contains(id)) {
            return Err(format!(
                "entity_attachments: type {bad} is a chest boat but not a boat"
            ));
        }
        if let Some(bad) = rafts.chain_find(&chest_rafts, |id| !boats.contains(id)) {
            return Err(format!(
                "entity_attachments: type {bad} is a raft but not a boat"
            ));
        }
        if let Some(bad) = camels.iter().find(|id| !horses.contains(id)) {
            return Err(format!(
                "entity_attachments: type {bad} is a camel but not an AbstractHorse — \
                 the most-derived-first order below depends on it"
            ));
        }

        let mut class = HashMap::new();
        for (&id, _) in by_id.iter() {
            // Most derived first: `Camel` before `AbstractHorse`, and the two
            // raft leaves before the `AbstractBoat` branch that reads them.
            let c = if boats.contains(&id) {
                VehicleClass::Boat {
                    raft: rafts.contains(&id) || chest_rafts.contains(&id),
                    chest: chest_boats.contains(&id),
                }
            } else if minecarts.contains(&id) {
                VehicleClass::Minecart
            } else if cube_mobs.contains(&id) {
                VehicleClass::CubeMob
            } else if striders.contains(&id) {
                VehicleClass::Strider
            } else if camels.contains(&id) {
                VehicleClass::Camel
            } else if horses.contains(&id) {
                VehicleClass::Horse
            } else {
                VehicleClass::Default
            };
            class.insert(id, c);
        }

        let mut forces_body_yaw = horses.clone();
        forces_body_yaw.extend(chickens.iter().copied());
        let living = set(classes::LIVING, "living")?;

        log::info!(
            "rewo-data: entity attachments — {} types, {} declaring seats, {} overriding \
             the derivation",
            by_id.len(),
            by_id.values().filter(|p| p.seats() > 0).count(),
            class.values().filter(|c| **c != VehicleClass::Default).count()
        );
        Ok(Self {
            by_id,
            class,
            animal,
            lowered_in_minecart,
            spider,
            forces_body_yaw,
            living,
        })
    }

    /// The declared points of a type, or `None` when the id is not a
    /// registered type.
    ///
    /// `None` rather than a default, on the discipline the swing table
    /// records: answering with the fallback point for an id the registry
    /// never had would be a guess dressed as a fact.
    pub fn points(&self, type_id: i32) -> Option<&TypePoints> {
        self.by_id.get(&type_id)
    }

    /// Which class's override this type dispatches to.
    pub fn class(&self, type_id: i32) -> VehicleClass {
        self.class
            .get(&type_id)
            .copied()
            .unwrap_or(VehicleClass::Default)
    }

    /// `passenger instanceof Animal`.
    pub fn is_animal(&self, type_id: i32) -> bool {
        self.animal.contains(&type_id)
    }

    /// `passenger instanceof Villager || passenger instanceof WanderingTrader`.
    pub fn lowers_in_minecart(&self, type_id: i32) -> bool {
        self.lowered_in_minecart.contains(&type_id)
    }

    /// `this instanceof Spider` — the rider side of `getVehicleAttachmentPoint`.
    pub fn is_spider(&self, type_id: i32) -> bool {
        self.spider.contains(&type_id)
    }

    /// Whether this **vehicle**'s `positionRider` copies its body yaw onto a
    /// living passenger (`AbstractHorse` and `Chicken`).
    pub fn forces_rider_body_yaw(&self, type_id: i32) -> bool {
        self.forces_body_yaw.contains(&type_id)
    }

    /// `passenger instanceof LivingEntity` — the cast that gates the body-yaw
    /// copy. A boat riding a chicken is not turned by it.
    pub fn is_living(&self, type_id: i32) -> bool {
        self.living.contains(&type_id)
    }

    /// Build from raw ids for **unit tests** in other crates that must not
    /// read the datagen reports. Nothing that ships calls it.
    pub fn from_raw(
        points: &[(i32, TypePoints)],
        class: &[(i32, VehicleClass)],
        animal: &[i32],
        lowered_in_minecart: &[i32],
        spider: &[i32],
        forces_body_yaw: &[i32],
        living: &[i32],
    ) -> Self {
        Self {
            by_id: points.iter().cloned().collect(),
            class: class.iter().copied().collect(),
            animal: animal.iter().copied().collect(),
            lowered_in_minecart: lowered_in_minecart.iter().copied().collect(),
            spider: spider.iter().copied().collect(),
            forces_body_yaw: forces_body_yaw.iter().copied().collect(),
            living: living.iter().copied().collect(),
        }
    }
}

/// Tiny helper so the two raft leaves can be checked in one expression.
trait ChainFind {
    fn chain_find<'a>(
        &'a self,
        other: &'a HashSet<i32>,
        f: impl Fn(&i32) -> bool,
    ) -> Option<&'a i32>;
}

impl ChainFind for HashSet<i32> {
    fn chain_find<'a>(
        &'a self,
        other: &'a HashSet<i32>,
        f: impl Fn(&i32) -> bool,
    ) -> Option<&'a i32> {
        self.iter().chain(other.iter()).find(|id| f(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(height: f32, passenger: &'static [[f64; 3]]) -> TypePoints {
        TypePoints {
            width: 0.6,
            height,
            passenger,
            vehicle: None,
        }
    }

    #[test]
    fn undeclared_passenger_point_is_the_at_height_fallback() {
        // `EntityAttachment.Fallback.AT_HEIGHT` — `(0, height, 0)`, the top of
        // the bounding box, NOT the feet. `height` is a Java `float` widened
        // into the `Vec3`'s double, so the widened value is the exact one and
        // `1.8f64` is not it.
        let want = [0.0, 1.8f32 as f64, 0.0];
        assert_eq!(pts(1.8, &[]).passenger_point(0), want);
        // And the fallback is a one-element list, so every index clamps onto it.
        assert_eq!(pts(1.8, &[]).passenger_point(7), want);
    }

    #[test]
    fn undeclared_vehicle_point_is_at_feet_not_at_height() {
        assert_eq!(pts(1.8, &[]).vehicle_point(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_nth_passenger_takes_the_nth_seat_and_clamps() {
        // The happy ghast's four seats.
        static SEATS: &[[f64; 3]] = &[
            [0.0, 4.0, 1.7],
            [-1.7, 4.0, 0.0],
            [0.0, 4.0, -1.7],
            [1.7, 4.0, 0.0],
        ];
        let p = pts(4.0, SEATS);
        assert_eq!(p.passenger_point(0), [0.0, 4.0, 1.7]);
        assert_eq!(p.passenger_point(1), [-1.7, 4.0, 0.0]);
        assert_eq!(p.passenger_point(3), [1.7, 4.0, 0.0]);
        // `getClamped` — a fifth rider takes the last seat, it does not wrap
        // to the first and it does not panic.
        assert_eq!(p.passenger_point(4), [1.7, 4.0, 0.0]);
        assert_eq!(p.passenger_point(99), [1.7, 4.0, 0.0]);
    }

    #[test]
    fn the_generated_table_carries_the_two_points_the_arithmetic_needs() {
        // Read straight off the generated table so a regenerated table that
        // dropped either column fails here rather than in a render.
        let by = |name: &str| {
            table::TYPES
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} missing from the generated table"))
        };
        // `passengerAttachments(0.86875F)` — a bare float is a **Y** offset.
        assert_eq!(by("minecraft:pig").passenger, &[[0.0, 0.86875, 0.0]]);
        // `Avatar.DEFAULT_VEHICLE_ATTACHMENT`, resolved through the symbol
        // lookup rather than inlined.
        assert_eq!(by("minecraft:player").vehicle, Some([0.0, 0.6, 0.0]));
        // `ridingOffset(-0.7F)` is **negated** into the VEHICLE point.
        assert_eq!(by("minecraft:zombie").vehicle, Some([0.0, 0.7, 0.0]));
        // Four seats, and `ridingOffset(0.5F)` → `(0, -0.5, 0)`.
        assert_eq!(by("minecraft:happy_ghast").passenger.len(), 4);
        assert_eq!(by("minecraft:happy_ghast").vehicle, Some([0.0, -0.5, 0.0]));
        // A boat and a camel declare none — both fully override the lookup, so
        // an empty list here is the expected state and not a parse failure.
        assert!(by("minecraft:oak_boat").passenger.is_empty());
        assert!(by("minecraft:camel").passenger.is_empty());
    }
}
