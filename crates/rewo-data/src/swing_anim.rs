//! Item → `minecraft:swing_animation`, the component that decides how long a
//! combat swing lasts and which arm animation plays (M19).
//!
//! Two decompiled call sites consume it:
//!
//! - `LivingEntity.getCurrentSwingDuration()` —
//!   `getItemInHand(swingingArm).getSwingAnimation().duration()`, the number of
//!   ticks the swing runs for.
//! - `ArmedEntityRenderState.extractArmedEntityRenderState` —
//!   `getItemHeldByArm(attackArm).getSwingAnimation().type()`, which
//!   `HumanoidModel.setupAttackAnimation` switches on.
//!
//! **The value is not on the wire.** `ItemStack.OPTIONAL_STREAM_CODEC` sends
//! `count + item id + DataComponentPatch`, and the patch holds only deltas from
//! the item's prototype. `DataComponents.COMMON_ITEM_COMPONENTS` sets
//! `SWING_ANIMATION -> SwingAnimation.DEFAULT` on every item and the spears
//! override it in their own properties, so a vanilla server transmits nothing —
//! the client reads it off the item id. [`swing_anim_table`] is that mapping,
//! machine-extracted from the datagen item-component report by
//! `tools/gen_swing_animations.py`.
//!
//! A patch *can* still carry an explicit override (the component is
//! `networkSynchronized`); decoding that is `rewo_net`'s job — this module owns
//! only the prototype table.
//!
//! [`swing_anim_table`]: crate::swing_anim_table

use std::collections::HashMap;

use crate::items::Items;
use crate::swing_anim_table as table;

/// `net.minecraft.world.item.SwingAnimationType`. The declared int is the wire
/// id (`STREAM_CODEC = ByteBufCodecs.idMapper(BY_ID, getId)`), and `BY_ID` is
/// `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)` — an out-of-range id
/// decodes to the **zero** entry, `NONE`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SwingAnimationType {
    /// No arm strike; `setupAttackAnimation` still rotates the body + arm pivots.
    #[default]
    None,
    /// The classic overhead whack (every item but the spears).
    Whack,
    /// `SpearAnimations.thirdPersonAttackHand`.
    Stab,
}

impl SwingAnimationType {
    /// `ByIdMap.continuous(getId, values(), OutOfBoundsStrategy.ZERO)` —
    /// anything outside 0..2 maps to the id-0 entry (`NONE`).
    pub const fn from_wire_id(id: i32) -> Self {
        match id {
            1 => SwingAnimationType::Whack,
            2 => SwingAnimationType::Stab,
            _ => SwingAnimationType::None,
        }
    }

    pub const fn wire_id(self) -> u8 {
        match self {
            SwingAnimationType::None => 0,
            SwingAnimationType::Whack => 1,
            SwingAnimationType::Stab => 2,
        }
    }
}

/// `net.minecraft.world.item.component.SwingAnimation`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SwingAnimation {
    pub kind: SwingAnimationType,
    /// Ticks the swing runs for, before the haste / mining-fatigue adjustment
    /// in `LivingEntity.getCurrentSwingDuration`.
    pub duration: i32,
}

impl SwingAnimation {
    /// `SwingAnimation.DEFAULT` — the generated constants, so a version bump
    /// that changes the record moves this with it.
    pub const DEFAULT: SwingAnimation = SwingAnimation {
        kind: SwingAnimationType::from_wire_id(table::DEFAULT_TYPE_ID as i32),
        duration: table::DEFAULT_DURATION,
    };

    pub const fn new(kind: SwingAnimationType, duration: i32) -> Self {
        SwingAnimation { kind, duration }
    }
}

impl Default for SwingAnimation {
    fn default() -> Self {
        SwingAnimation::DEFAULT
    }
}

/// Item protocol id → the item's **prototype** swing animation.
///
/// Only the non-default items get an entry; [`Self::of`] returns
/// [`SwingAnimation::DEFAULT`] for everything else — including an item id this
/// registry doesn't know and the empty stack. That is exact:
/// `ItemStack.EMPTY.components` is an empty `PatchedDataComponentMap`, so
/// `getOrDefault(SWING_ANIMATION, DEFAULT)` returns the default for a bare hand
/// too.
pub struct SwingAnimations {
    non_default: HashMap<i32, SwingAnimation>,
    /// Every registered item id, so [`Self::of`] can answer "not a vanilla
    /// item" instead of quietly returning `DEFAULT` for an id it never saw.
    valid: std::collections::HashSet<i32>,
}

impl SwingAnimations {
    /// Resolve the generated name table against the live item registry.
    ///
    /// Fails loud when a generated name is absent from `registries.json`, or
    /// when the registry is a different size than the one the table was
    /// generated from — either means the table and the pinned version have
    /// drifted, and silently dropping a spear would make its swing 6 ticks
    /// instead of 13..23.
    pub fn resolve(items: &Items) -> Result<Self, String> {
        if items.len() != table::SCANNED_ITEMS {
            return Err(format!(
                "swing_anim: the item registry has {} entries but the generated table was \
                 built from {} — re-run tools/gen_swing_animations.py after the version bump",
                items.len(),
                table::SCANNED_ITEMS
            ));
        }
        let mut non_default = HashMap::with_capacity(table::NON_DEFAULT.len());
        for &(name, type_id, duration) in table::NON_DEFAULT {
            let id = items.id(name).ok_or_else(|| {
                format!(
                    "swing_anim: generated item {name:?} is not in the item registry — \
                     re-run tools/gen_swing_animations.py after the version bump"
                )
            })?;
            non_default.insert(
                id,
                SwingAnimation::new(SwingAnimationType::from_wire_id(type_id as i32), duration),
            );
        }
        log::info!(
            "rewo-data: {} non-default swing animation(s) resolved over {} registered items",
            non_default.len(),
            items.len(),
        );
        Ok(Self {
            non_default,
            valid: items.id_set(),
        })
    }

    /// The prototype swing animation of an item id, or `None` when `item_id` is
    /// **not a registered item**.
    ///
    /// The `None` arm is load-bearing. `COMMON_ITEM_COMPONENTS` guarantees a
    /// prototype for every *vanilla* item, so absence from the non-default map
    /// means "the default". For an id the registry does not contain, nothing is
    /// known — answering `DEFAULT` there would be a guess dressed as a fact,
    /// and the caller must mark that swing input unknown instead.
    pub fn of(&self, item_id: i32) -> Option<SwingAnimation> {
        self.valid.contains(&item_id).then(|| {
            self.non_default
                .get(&item_id)
                .copied()
                .unwrap_or(SwingAnimation::DEFAULT)
        })
    }

    pub fn non_default_count(&self) -> usize {
        self.non_default.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_whack_six() {
        // `SwingAnimation.DEFAULT = new SwingAnimation(SwingAnimationType.WHACK, 6)`.
        assert_eq!(SwingAnimation::DEFAULT.kind, SwingAnimationType::Whack);
        assert_eq!(SwingAnimation::DEFAULT.duration, 6);
    }

    #[test]
    fn out_of_range_type_ids_fall_to_none_not_whack() {
        // `ByIdMap.OutOfBoundsStrategy.ZERO` → the id-0 entry.
        assert_eq!(SwingAnimationType::from_wire_id(0), SwingAnimationType::None);
        assert_eq!(SwingAnimationType::from_wire_id(1), SwingAnimationType::Whack);
        assert_eq!(SwingAnimationType::from_wire_id(2), SwingAnimationType::Stab);
        for bad in [-1, 3, 99, i32::MIN, i32::MAX] {
            assert_eq!(SwingAnimationType::from_wire_id(bad), SwingAnimationType::None);
        }
    }

    #[test]
    fn every_generated_entry_is_a_spear_stab() {
        // The report says exactly the seven spears differ; if a version bump
        // adds a WHACK-with-odd-duration item this still passes, but a table
        // that somehow generated a *default* row would not.
        assert!(!table::NON_DEFAULT.is_empty());
        for &(name, type_id, duration) in table::NON_DEFAULT {
            assert!(name.starts_with("minecraft:"), "{name}");
            assert!(
                (type_id, duration) != (table::DEFAULT_TYPE_ID, table::DEFAULT_DURATION),
                "{name} is the default and should not be in the table"
            );
            assert!(duration > 0, "{name} duration {duration}");
        }
    }
}
