//! The player's inventory (M34).
//!
//! Rewo could dig, place, swing and render a held item long before it knew what
//! was in any slot: `set_equipment` tells it what *other* entities hold, but
//! nothing told it what *you* hold. That is why `WorldRenderer::set_hud` takes
//! `(health, food, slot)` and draws nine empty boxes, and why the local player's
//! hand is empty in first person.
//!
//! # The two coordinate systems
//!
//! Vanilla has two, and mixing them puts your pickaxe in an armour slot.
//!
//! - **Menu slots** are what the wire speaks. `InventoryMenu` (container 0) is
//!   46 slots: `0` the crafting result, `1..5` the 2×2 grid, `5..9` armour,
//!   `9..36` the main inventory, `36..45` the hotbar, `45` the off-hand.
//! - **Inventory indices** are what the game logic speaks: `0..9` the hotbar,
//!   `9..36` main, with the off-hand off at index 40.
//!
//! So the hotbar is `36 + i` as a menu slot and `i` as an inventory index, and
//! `set_held_slot` speaks the *second* one. [`Inventory::hotbar`] is the only
//! place that conversion happens.
//!
//! # Why a bad packet drops whole
//!
//! `container_set_content` is a length-prefixed list of `ItemStack`s, each of
//! which carries a `DataComponentPatch` whose values are encoded with per-type
//! codecs. `rewo_net::item_stack` walks the handful of components it knows and
//! reports [`aligned`](rewo_net::item_stack::WireSlot::aligned) — `false` means
//! the reader is parked mid-value and every *following* slot is garbage.
//!
//! Vanilla throws a `DecoderException` there, which drops the connection. Rewo
//! keeps the connection and **discards the whole packet**, leaving the previous
//! contents in place. Half-applying would be the one genuinely bad option: it
//! would show a confident, wrong inventory.

/// One occupied slot. An empty slot is `None`, matching `ItemStack.EMPTY`
/// rather than a count of zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemSlot {
    /// Item registry protocol id, exactly as sent.
    pub item_id: i32,
    pub count: i32,
}

/// `InventoryMenu` (container 0) — the 46-slot menu the server synchronises.
pub const MENU_SLOTS: usize = 46;
/// `InventoryMenu.USE_ROW_SLOT_START` — the hotbar's first menu slot.
pub const HOTBAR_MENU_START: usize = 36;
/// `Inventory.SELECTION_SIZE`.
pub const HOTBAR_SIZE: usize = 9;
/// The off-hand's menu slot (the one past the hotbar's end).
pub const OFFHAND_MENU_SLOT: usize = 45;
/// `InventoryMenu.ARMOR_SLOT_START` — helmet first, boots last.
pub const ARMOR_MENU_START: usize = 5;

/// `InventoryMenu.CONTAINER_ID`. Every other container id belongs to an open
/// screen, which Rewo has none of.
pub const PLAYER_CONTAINER_ID: i32 = 0;

#[derive(Clone, Debug)]
pub struct Inventory {
    slots: [Option<ItemSlot>; MENU_SLOTS],
    /// The stack on the cursor. Decoded because the packet carries it; nothing
    /// renders it yet (it is only visible with an inventory screen open).
    carried: Option<ItemSlot>,
    /// `Inventory.selectedSlot`, an **inventory index** in `0..9`.
    selected: u8,
    /// The server's `stateId` for the last content/slot update applied. Kept
    /// for diagnostics and for the eventual serverbound click, which must echo
    /// it; nothing reads it yet.
    state_id: i32,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: [None; MENU_SLOTS],
            carried: None,
            selected: 0,
            state_id: 0,
        }
    }
}

impl Inventory {
    /// `Inventory.isHotbarSlot` — an **inventory index** test.
    pub fn is_hotbar_index(index: i32) -> bool {
        (0..HOTBAR_SIZE as i32).contains(&index)
    }

    /// `InventoryMenu.isHotbarSlot` — a **menu slot** test. Note the asymmetry
    /// with the above: same name in vanilla, different coordinate system.
    pub fn is_hotbar_menu_slot(slot: i32) -> bool {
        (HOTBAR_MENU_START as i32..(HOTBAR_MENU_START + HOTBAR_SIZE) as i32).contains(&slot)
    }

    /// Replace the whole container. Rejects a wrong-length list rather than
    /// padding or truncating: a server that sends 46 slots for container 0 and
    /// something else for anything else is telling us which container it means.
    pub fn set_content(
        &mut self,
        state_id: i32,
        slots: &[Option<ItemSlot>],
        carried: Option<ItemSlot>,
    ) -> bool {
        if slots.len() != MENU_SLOTS {
            return false;
        }
        self.slots.copy_from_slice(slots);
        self.carried = carried;
        self.state_id = state_id;
        true
    }

    /// `handleContainerSetSlot` for container 0. An out-of-range slot is
    /// ignored, not clamped.
    pub fn set_slot(&mut self, state_id: i32, slot: i32, item: Option<ItemSlot>) -> bool {
        let Ok(idx) = usize::try_from(slot) else {
            return false;
        };
        if idx >= MENU_SLOTS {
            return false;
        }
        self.slots[idx] = item;
        self.state_id = state_id;
        true
    }

    /// `handleSetHeldSlot`. **An out-of-range slot is ignored**, exactly as
    /// vanilla's `if (Inventory.isHotbarSlot(...))` guard does — it does not
    /// clamp, and it does not reset to zero.
    pub fn set_selected(&mut self, index: i32) -> bool {
        if !Self::is_hotbar_index(index) {
            return false;
        }
        self.selected = index as u8;
        true
    }

    /// The selected hotbar index, `0..9`.
    pub fn selected(&self) -> u8 {
        self.selected
    }

    pub fn state_id(&self) -> i32 {
        self.state_id
    }

    pub fn carried(&self) -> Option<ItemSlot> {
        self.carried
    }

    /// One hotbar slot by **inventory index** (`0..9`) — the conversion to a
    /// menu slot happens here and nowhere else.
    pub fn hotbar(&self, index: usize) -> Option<ItemSlot> {
        if index >= HOTBAR_SIZE {
            return None;
        }
        self.slots[HOTBAR_MENU_START + index]
    }

    /// What the player is holding in the main hand.
    pub fn held(&self) -> Option<ItemSlot> {
        self.hotbar(self.selected as usize)
    }

    /// The off-hand stack.
    pub fn offhand(&self) -> Option<ItemSlot> {
        self.slots[OFFHAND_MENU_SLOT]
    }

    /// One armour slot, `0` helmet through `3` boots.
    pub fn armor(&self, index: usize) -> Option<ItemSlot> {
        if index >= 4 {
            return None;
        }
        self.slots[ARMOR_MENU_START + index]
    }

    /// Raw menu-slot access, for the gate and for diagnostics.
    pub fn menu_slot(&self, slot: usize) -> Option<ItemSlot> {
        self.slots.get(slot).copied().flatten()
    }

    /// Whether anything at all has arrived. Before the first
    /// `container_set_content` the hotbar is legitimately empty, and that is
    /// different from "the player is holding nothing".
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none()) && self.carried.is_none()
    }

    /// Drop everything, for a dimension change or a respawn — the server
    /// re-sends the contents, and stale slots would show through until it did.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(id: i32, n: i32) -> Option<ItemSlot> {
        Some(ItemSlot {
            item_id: id,
            count: n,
        })
    }

    /// The hotbar is menu slots 36..45 and inventory indices 0..9. Getting this
    /// backwards is the whole reason the conversion lives in one method.
    #[test]
    fn the_hotbar_bridges_the_two_coordinate_systems() {
        let mut inv = Inventory::default();
        let mut slots = [None; MENU_SLOTS];
        slots[HOTBAR_MENU_START] = stack(1, 1); // hotbar index 0
        slots[HOTBAR_MENU_START + 8] = stack(9, 9); // hotbar index 8
        slots[0] = stack(99, 1); // the crafting RESULT, not a hotbar slot
        assert!(inv.set_content(0, &slots, None));

        assert_eq!(inv.hotbar(0).unwrap().item_id, 1);
        assert_eq!(inv.hotbar(8).unwrap().item_id, 9);
        assert_eq!(inv.hotbar(9), None, "there is no tenth hotbar slot");
        assert_eq!(
            inv.menu_slot(0).unwrap().item_id,
            99,
            "menu slot 0 is the crafting result and must not read as hotbar 0"
        );
    }

    /// `set_held_slot` speaks inventory indices, and an out-of-range one is
    /// IGNORED — vanilla's guard does not clamp.
    #[test]
    fn an_out_of_range_held_slot_is_ignored_not_clamped() {
        let mut inv = Inventory::default();
        assert!(inv.set_selected(4));
        assert_eq!(inv.selected(), 4);
        for bad in [-1, 9, 36, 100] {
            assert!(!inv.set_selected(bad), "{bad} should be rejected");
            assert_eq!(inv.selected(), 4, "and must leave the selection alone");
        }
    }

    /// `held()` composes the selection with the hotbar, so changing either
    /// changes what the player is holding.
    #[test]
    fn held_follows_both_the_selection_and_the_slot() {
        let mut inv = Inventory::default();
        let mut slots = [None; MENU_SLOTS];
        slots[HOTBAR_MENU_START + 2] = stack(276, 1);
        inv.set_content(0, &slots, None);
        assert_eq!(inv.held(), None, "selection is still 0");
        inv.set_selected(2);
        assert_eq!(inv.held().unwrap().item_id, 276);
        // And a single-slot update moves it.
        inv.set_slot(1, (HOTBAR_MENU_START + 2) as i32, stack(64, 3));
        assert_eq!(inv.held().unwrap(), ItemSlot { item_id: 64, count: 3 });
    }

    /// A wrong-length content list is rejected whole rather than padded.
    #[test]
    fn a_wrong_length_content_list_is_rejected() {
        let mut inv = Inventory::default();
        assert!(!inv.set_content(0, &[None; 10], None));
        assert!(!inv.set_content(0, &[None; 47], None));
        assert!(inv.set_content(0, &[None; MENU_SLOTS], None));
    }

    #[test]
    fn an_out_of_range_slot_update_is_ignored() {
        let mut inv = Inventory::default();
        inv.set_content(0, &[None; MENU_SLOTS], None);
        assert!(!inv.set_slot(1, -1, stack(1, 1)));
        assert!(!inv.set_slot(1, MENU_SLOTS as i32, stack(1, 1)));
        assert!(inv.set_slot(1, 45, stack(1, 1)));
        assert_eq!(inv.offhand().unwrap().item_id, 1);
    }

    /// The armour block is helmet-first, and it must not overlap the crafting
    /// grid below it or the main inventory above it.
    #[test]
    fn armour_is_the_four_slots_from_five() {
        let mut inv = Inventory::default();
        let mut slots = [None; MENU_SLOTS];
        for i in 0..4 {
            slots[ARMOR_MENU_START + i] = stack(100 + i as i32, 1);
        }
        slots[4] = stack(7, 1); // last crafting-grid slot
        slots[9] = stack(8, 1); // first main-inventory slot
        inv.set_content(0, &slots, None);
        for i in 0..4 {
            assert_eq!(inv.armor(i).unwrap().item_id, 100 + i as i32);
        }
        assert_eq!(inv.armor(4), None);
    }

    /// A dimension change must not leave the old world's hotbar on screen.
    #[test]
    fn clearing_empties_everything() {
        let mut inv = Inventory::default();
        let mut slots = [None; MENU_SLOTS];
        slots[HOTBAR_MENU_START] = stack(1, 1);
        inv.set_content(7, &slots, stack(2, 1));
        inv.set_selected(3);
        assert!(!inv.is_empty());
        inv.clear();
        assert!(inv.is_empty());
        assert_eq!(inv.selected(), 0);
        assert_eq!(inv.carried(), None);
        assert_eq!(inv.state_id(), 0);
    }
}
