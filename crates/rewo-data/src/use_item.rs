//! Item → `getUseDuration` / `getUseAnimation`, the two values that decide
//! which *use-driven* arm pose an entity holds (M23).
//!
//! Three decompiled call sites consume them:
//!
//! - `LivingEntity.onSyncedDataUpdated` — when the `DATA_LIVING_ENTITY_FLAGS`
//!   "using" bit flips on, the client sets
//!   `useItemRemaining = useItem.getUseDuration(this)`. **That is the whole
//!   reason this module exists**: the remaining-tick counter is never sent, it
//!   is reconstructed from the item id.
//! - `AvatarRenderer.getArmPose` — switches on `itemInHand.getUseAnimation()`
//!   for the eight poses gated behind `getUseItemRemainingTicks() > 0`.
//! - `LivingEntity.getTicksUsingItem()` —
//!   `useItem.getUseDuration(this) - useItemRemaining`, the elapsed count the
//!   crossbow-charge pose lerps over.
//!
//! **Neither value is on the wire.** Both are computed from prototype data
//! components (`minecraft:consumable`, `minecraft:blocks_attacks`,
//! `minecraft:kinetic_weapon`) that a vanilla server never puts in a
//! `DataComponentPatch`, plus eight item classes that override the methods
//! outright. [`use_item_table`] is that mapping, machine-extracted by
//! `tools/gen_use_items.py`.
//!
//! Same caveat as [`crate::swing_anim`]: a patch *could* carry an explicit
//! `consumable` override, and this module does not model that — it owns only
//! the prototype table. `rewo_net`'s patch walk already fails closed on any
//! component whose codec it cannot step over, so a stack that patched one would
//! be reported unknowable rather than silently mis-resolved.
//!
//! [`use_item_table`]: crate::use_item_table

use std::collections::HashMap;

use crate::items::Items;
use crate::use_item_table as table;

/// `net.minecraft.world.item.ItemUseAnimation`. The declared int is the wire id
/// (`STREAM_CODEC = ByteBufCodecs.idMapper(BY_ID, getId)`), and `BY_ID` is
/// `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)` — an out-of-range id
/// decodes to the **zero** entry, `NONE`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ItemUseAnimation {
    /// Not usable at all. No use-driven pose is reachable.
    #[default]
    None,
    Eat,
    Drink,
    /// Shield. → `ArmPose::Block`.
    Block,
    /// Bow. → `ArmPose::BowAndArrow`.
    Bow,
    /// Trident. → `ArmPose::ThrowTrident`.
    Trident,
    /// Crossbow being charged. → `ArmPose::CrossbowCharge`.
    Crossbow,
    /// Spyglass. → `ArmPose::Spyglass`.
    Spyglass,
    /// Goat horn. → `ArmPose::TootHorn`.
    TootHorn,
    /// Brush. → `ArmPose::Brush`.
    Brush,
    /// Bundle. **Not** an arm pose — `getArmPose` has no `BUNDLE` case, so a
    /// bundle in use falls through to the spear/item tail like any other item.
    Bundle,
    /// Spear. → `ArmPose::Spear`.
    Spear,
}

impl ItemUseAnimation {
    /// `ByIdMap.continuous(getId, values(), OutOfBoundsStrategy.ZERO)` —
    /// anything outside 0..11 maps to the id-0 entry (`NONE`).
    pub const fn from_wire_id(id: i32) -> Self {
        match id {
            1 => ItemUseAnimation::Eat,
            2 => ItemUseAnimation::Drink,
            3 => ItemUseAnimation::Block,
            4 => ItemUseAnimation::Bow,
            5 => ItemUseAnimation::Trident,
            6 => ItemUseAnimation::Crossbow,
            7 => ItemUseAnimation::Spyglass,
            8 => ItemUseAnimation::TootHorn,
            9 => ItemUseAnimation::Brush,
            10 => ItemUseAnimation::Bundle,
            11 => ItemUseAnimation::Spear,
            _ => ItemUseAnimation::None,
        }
    }

    pub const fn wire_id(self) -> u8 {
        match self {
            ItemUseAnimation::None => 0,
            ItemUseAnimation::Eat => 1,
            ItemUseAnimation::Drink => 2,
            ItemUseAnimation::Block => 3,
            ItemUseAnimation::Bow => 4,
            ItemUseAnimation::Trident => 5,
            ItemUseAnimation::Crossbow => 6,
            ItemUseAnimation::Spyglass => 7,
            ItemUseAnimation::TootHorn => 8,
            ItemUseAnimation::Brush => 9,
            ItemUseAnimation::Bundle => 10,
            ItemUseAnimation::Spear => 11,
        }
    }
}

/// What one item does when used: how long, and with which animation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct UseProfile {
    /// `getUseDuration`, in ticks. `0` means the item cannot be used —
    /// `LivingEntity.startUsingItem` still sets the flag, but
    /// `useItemRemaining` is immediately 0 so `getUseItemRemainingTicks() > 0`
    /// is false and no use pose is reachable.
    pub duration: i32,
    /// `getUseAnimation`.
    pub animation: ItemUseAnimation,
}

impl UseProfile {
    /// The answer for an item with none of the three components and no class
    /// override — `(0, NONE)`, i.e. "cannot be used".
    pub const UNUSABLE: UseProfile = UseProfile {
        duration: 0,
        animation: ItemUseAnimation::None,
    };
}

/// Item protocol id → the item's **prototype** use profile.
///
/// Only usable items get an entry; [`Self::of`] returns [`UseProfile::UNUSABLE`]
/// for everything else — including the empty stack, which is exact:
/// `ItemStack.EMPTY` has no components, so the base rule's final `else` returns
/// `0` / `NONE` for a bare hand too.
pub struct UseProfiles {
    usable: HashMap<i32, UseProfile>,
    /// Every registered item id, so [`Self::of`] can answer "not a vanilla
    /// item" instead of quietly claiming an unknown id is unusable.
    valid: std::collections::HashSet<i32>,
}

impl UseProfiles {
    /// Resolve the generated name table against the live item registry.
    ///
    /// Fails loud when a generated name is absent from `registries.json`, or
    /// when the registry is a different size than the one the table was
    /// generated from — either means the table and the pinned version have
    /// drifted, and silently dropping the shield would leave a blocking player
    /// holding their arm at the ordinary item angle.
    pub fn resolve(items: &Items) -> Result<Self, String> {
        if items.len() != table::SCANNED_ITEMS {
            return Err(format!(
                "use_item: the item registry has {} entries but the generated table was \
                 built from {} — re-run tools/gen_use_items.py after the version bump",
                items.len(),
                table::SCANNED_ITEMS
            ));
        }
        let mut usable = HashMap::with_capacity(table::USABLE.len());
        for &(name, duration, anim_id) in table::USABLE {
            let id = items.id(name).ok_or_else(|| {
                format!(
                    "use_item: generated item {name:?} is not in the item registry — \
                     re-run tools/gen_use_items.py after the version bump"
                )
            })?;
            usable.insert(
                id,
                UseProfile {
                    duration,
                    animation: ItemUseAnimation::from_wire_id(anim_id as i32),
                },
            );
        }
        log::info!(
            "rewo-data: {} usable item(s) resolved over {} registered items",
            usable.len(),
            items.len(),
        );
        Ok(Self {
            usable,
            valid: items.id_set(),
        })
    }

    /// The prototype use profile of an item id, or `None` when `item_id` is
    /// **not a registered item**.
    ///
    /// The `None` arm is load-bearing for the same reason it is in
    /// [`crate::swing_anim::SwingAnimations::of`]: for an id the registry does
    /// not contain, nothing is known, and answering "unusable" would be a guess
    /// dressed as a fact. The caller suppresses the use pose instead.
    pub fn of(&self, item_id: i32) -> Option<UseProfile> {
        self.valid
            .contains(&item_id)
            .then(|| self.usable.get(&item_id).copied().unwrap_or(UseProfile::UNUSABLE))
    }

    pub fn usable_count(&self) -> usize {
        self.usable.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_ids_fall_to_none() {
        // `ByIdMap.OutOfBoundsStrategy.ZERO` → the id-0 entry.
        assert_eq!(ItemUseAnimation::from_wire_id(0), ItemUseAnimation::None);
        assert_eq!(ItemUseAnimation::from_wire_id(12), ItemUseAnimation::None);
        assert_eq!(ItemUseAnimation::from_wire_id(-1), ItemUseAnimation::None);
        assert_eq!(ItemUseAnimation::from_wire_id(99), ItemUseAnimation::None);
    }

    #[test]
    fn wire_ids_round_trip() {
        for id in 0..=11 {
            assert_eq!(ItemUseAnimation::from_wire_id(id).wire_id() as i32, id);
        }
    }

    #[test]
    fn generated_table_matches_the_decompiled_literals() {
        // Spot-checks straight off the decompile, so a regenerated table that
        // silently changed one of these fails here rather than in a render.
        let by_name = |want: &str| {
            table::USABLE
                .iter()
                .find(|(n, _, _)| *n == want)
                .map(|&(_, d, a)| (d, a))
        };
        // `BowItem` 72000 / BOW, `SpyglassItem` 1200 / SPYGLASS,
        // `BrushItem` 200 / BRUSH, `TridentItem` 72000 / TRIDENT.
        assert_eq!(by_name("minecraft:bow"), Some((72000, table::anim::BOW)));
        assert_eq!(
            by_name("minecraft:spyglass"),
            Some((1200, table::anim::SPYGLASS))
        );
        assert_eq!(by_name("minecraft:brush"), Some((200, table::anim::BRUSH)));
        assert_eq!(
            by_name("minecraft:trident"),
            Some((72000, table::anim::TRIDENT))
        );
        // The shield reaches BLOCK through the *base* rule's `BLOCKS_ATTACKS`
        // branch, not an override — it is the only item with that component.
        assert_eq!(
            by_name("minecraft:shield"),
            Some((table::BLOCKING_DURATION, table::anim::BLOCK))
        );
        // A spear reaches SPEAR through `KINETIC_WEAPON`, also base-rule.
        assert_eq!(
            by_name("minecraft:iron_spear"),
            Some((table::BLOCKING_DURATION, table::anim::SPEAR))
        );
        // `Consumable.consumeTicks()` = `(int)(consumeSeconds * 20)`; the
        // golden apple takes the 1.6 s default → 32.
        assert_eq!(
            by_name("minecraft:golden_apple"),
            Some((32, table::anim::EAT))
        );
        // Every goat-horn instrument declares `useDuration 7.0F` → 140.
        assert_eq!(
            by_name("minecraft:goat_horn"),
            Some((140, table::anim::TOOT_HORN))
        );
        // `EnderEyeItem.getUseDuration` returns 0, so it is *absent* — an
        // override to zero must not leave it usable.
        assert_eq!(by_name("minecraft:ender_eye"), None);
    }

    #[test]
    fn unusable_is_the_empty_hand_answer() {
        assert_eq!(UseProfile::UNUSABLE.duration, 0);
        assert_eq!(UseProfile::UNUSABLE.animation, ItemUseAnimation::None);
    }
}
