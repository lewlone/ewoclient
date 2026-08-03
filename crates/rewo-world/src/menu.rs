//! The currently open container menu.
//!
//! Vanilla keeps two menus on the player at once and the distinction is
//! load-bearing for routing:
//!
//! * `player.inventoryMenu` — the 46-slot `InventoryMenu`, **container id 0**,
//!   which exists for the whole session and is never opened or closed by a
//!   packet.
//! * `player.containerMenu` — whatever is open. It *is* `inventoryMenu` when
//!   nothing is open, and is replaced by the container's menu when
//!   `open_screen` arrives.
//!
//! So a server can address either at any time, and `handleContainerSetSlot`
//! does exactly that: id 0 always writes `inventoryMenu`, any other id writes
//! `containerMenu` only if it matches. This type is the second of the two;
//! `Inventory` remains the first.
//!
//! What is deliberately *not* here yet: the container's own item slots. Those
//! arrive when `Inventory` becomes a layout-driven `Menu` (the next step), at
//! which point this grows a slot vector. Splitting it that way keeps the
//! packet decode landable without touching the 152-witness `inventoryshot`
//! surface.

use crate::menu_layout::{layout_of, MenuLayout};

/// The number of data slots any vanilla menu declares.
///
/// `BeaconMenu` checks 3, `AbstractFurnaceMenu` 4, `EnchantmentMenu` 10 (three
/// costs, three enchantment ids, three levels, and the seed) — the largest in
/// the registry. The array is fixed at that size rather than grown on demand
/// because a data id is an index vanilla never bounds-checks on the client
/// (`ContainerData.set` is backed by a fixed array per menu), so an
/// out-of-range id is a malformed packet rather than a bigger menu.
pub const MAX_DATA_SLOTS: usize = 10;

/// An open container menu.
#[derive(Debug, Clone)]
pub struct OpenMenu {
    /// The id the server addresses this menu by. Never 0.
    pub container_id: i32,
    /// The resolved layout. `open_screen` carries a raw registry id; a menu
    /// type Rewo has no layout for never becomes an `OpenMenu` at all.
    pub layout: &'static MenuLayout,
    /// The flattened title text.
    pub title: String,
    /// `container_set_data` values, indexed by data slot id.
    pub data: [i16; MAX_DATA_SLOTS],
    /// This menu's slots — the same [`crate::inventory::Inventory`] type the
    /// player's menu is, sized by `layout`.
    ///
    /// One type for both is the point of the generalization: the click
    /// arithmetic that `container_set_content` feeds is the same code
    /// whichever menu it lands in, so it cannot drift between them.
    pub menu: crate::inventory::Inventory,
}

impl OpenMenu {
    /// Read a data slot. Out-of-range ids read 0 rather than panicking — the
    /// value is only ever a progress bar's numerator.
    pub fn data(&self, id: i16) -> i16 {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.data.get(i))
            .copied()
            .unwrap_or(0)
    }
}

/// A furnace's four data slots, as `AbstractFurnaceMenu` names them.
///
/// `container_set_data` carries them by index; these are what the indices
/// mean.
pub const FURNACE_LIT_REMAINING: i16 = 0;
pub const FURNACE_LIT_DURATION: i16 = 1;
pub const FURNACE_COOK_PROGRESS: i16 = 2;
pub const FURNACE_COOK_TOTAL: i16 = 3;

/// `AbstractFurnaceMenu`'s three derived quantities (M91).
///
/// Two of the three have an edge case that a plain division gets wrong, and
/// both are pinned by witnesses:
///
/// * `getLitProgress` divides by `data[1]`, but **substitutes 200 when it is
///   zero** — a real fallback, not a guard. Dividing by zero gives infinity,
///   which clamps to 1.0 and paints a permanently full flame on a furnace that
///   has never been lit.
/// * `getBurnProgress` returns 0 unless **both** `data[2]` and `data[3]` are
///   non-zero. The `total != 0` half is what avoids the division; the
///   `current != 0` half is mathematically redundant (0/total is already 0)
///   and is transcribed anyway, because it is what the source says.
impl OpenMenu {
    /// `isLit()` — any lit time remaining.
    pub fn furnace_is_lit(&self) -> bool {
        self.data(FURNACE_LIT_REMAINING) > 0
    }

    /// `getLitProgress()` — how much of the current fuel is left, 0..=1.
    pub fn furnace_lit_progress(&self) -> f32 {
        let mut duration = self.data(FURNACE_LIT_DURATION) as f32;
        if duration == 0.0 {
            duration = 200.0;
        }
        (self.data(FURNACE_LIT_REMAINING) as f32 / duration).clamp(0.0, 1.0)
    }

    /// `getBurnProgress()` — how far the current item has cooked, 0..=1.
    pub fn furnace_burn_progress(&self) -> f32 {
        let current = self.data(FURNACE_COOK_PROGRESS) as f32;
        let total = self.data(FURNACE_COOK_TOTAL) as f32;
        if total != 0.0 && current != 0.0 {
            (current / total).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// The client's menu slot. Vanilla has exactly one — `Gui.screen` is a single
/// field and `setScreen` replaces it (M82's finding for the screen framework
/// generalises here), so this is an `Option`, not a stack.
#[derive(Debug, Clone, Default)]
pub struct Menus {
    open: Option<OpenMenu>,
}

impl Menus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self) -> Option<&OpenMenu> {
        self.open.as_ref()
    }

    pub fn open_mut(&mut self) -> Option<&mut OpenMenu> {
        self.open.as_mut()
    }

    /// Apply `open_screen`.
    ///
    /// Returns `false` and changes nothing when the menu type has no layout —
    /// `MenuScreens.create` logs a warning and does nothing for an
    /// unregistered type, so whatever was open stays open. Substituting a
    /// default layout would be worse than showing nothing: the slot indices
    /// would be wrong, and the player would be clicking slots that are not
    /// where they appear to be.
    pub fn apply_open_screen(&mut self, container_id: i32, menu_type: i32, title: String) -> bool {
        let Some(layout) = layout_of(menu_type) else {
            return false;
        };
        self.open = Some(OpenMenu {
            container_id,
            layout,
            title,
            data: [0; MAX_DATA_SLOTS],
            menu: crate::inventory::Inventory::with_layout(layout),
        });
        true
    }

    /// The open menu's slots, if its id matches.
    ///
    /// `handleContainerContent` and `handleContainerSetSlot` both gate on
    /// `packet.containerId() == player.containerMenu.containerId`, so a write
    /// addressed to a stale or unknown container is dropped rather than
    /// applied to whatever happens to be open.
    pub fn menu_for(&mut self, container_id: i32) -> Option<&mut crate::inventory::Inventory> {
        let m = self.open.as_mut()?;
        (m.container_id == container_id).then_some(&mut m.menu)
    }

    /// Apply `container_set_data`, gated the way `handleContainerSetData` is:
    /// only when the id matches the open menu.
    ///
    /// Returns whether it was applied.
    pub fn apply_set_data(&mut self, container_id: i32, id: i16, value: i16) -> bool {
        let Some(menu) = self.open.as_mut() else {
            return false;
        };
        if menu.container_id != container_id {
            return false;
        }
        let Ok(i) = usize::try_from(id) else {
            return false;
        };
        let Some(slot) = menu.data.get_mut(i) else {
            return false;
        };
        *slot = value;
        true
    }

    /// Apply `container_close`.
    ///
    /// **The id is read and ignored.** `handleContainerClose` closes whatever
    /// is open without comparing it, so a close for a stale id still closes
    /// the current menu — matching vanilla rather than second-guessing it.
    pub fn apply_close(&mut self) {
        self.open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_known_menu_resolves_its_layout() {
        let mut m = Menus::new();
        assert!(m.apply_open_screen(3, 2, "Chest".into()));
        let open = m.open().unwrap();
        assert_eq!(open.container_id, 3);
        assert_eq!(open.layout.name, "generic_9x3");
        assert_eq!(open.layout.slot_count(), 27 + 36);
    }

    #[test]
    fn an_unknown_menu_type_opens_nothing_and_leaves_the_previous_menu() {
        let mut m = Menus::new();
        assert!(m.apply_open_screen(1, 5, "Big".into()));
        // 25 is one past the end of the registry.
        assert!(!m.apply_open_screen(2, 25, "Mystery".into()));
        let open = m.open().expect("the previous menu must survive");
        assert_eq!(open.container_id, 1, "the failed open must not replace it");
        assert_eq!(open.layout.name, "generic_9x6");
    }

    #[test]
    fn set_data_only_applies_to_the_matching_container() {
        let mut m = Menus::new();
        m.apply_open_screen(7, 14, "Furnace".into());
        assert!(m.apply_set_data(7, 2, 100), "matching id applies");
        assert_eq!(m.open().unwrap().data(2), 100);
        assert!(!m.apply_set_data(8, 2, 55), "a different id is dropped");
        assert_eq!(m.open().unwrap().data(2), 100, "and changes nothing");
    }

    #[test]
    fn set_data_with_no_menu_open_is_inert() {
        let mut m = Menus::new();
        assert!(!m.apply_set_data(1, 0, 5));
        assert!(m.open().is_none());
    }

    #[test]
    fn a_negative_or_oversized_data_id_is_rejected_not_wrapped() {
        let mut m = Menus::new();
        m.apply_open_screen(1, 14, "Furnace".into());
        assert!(!m.apply_set_data(1, -1, 9));
        assert!(!m.apply_set_data(1, MAX_DATA_SLOTS as i16, 9));
        assert_eq!(m.open().unwrap().data(0), 0);
    }

    #[test]
    fn an_open_menu_carries_slots_sized_by_its_layout() {
        let mut m = Menus::new();
        m.apply_open_screen(3, 5, "Double Chest".into());
        assert_eq!(m.open().unwrap().menu.slot_count(), 54 + 36);
        m.apply_open_screen(4, 17, "Lectern".into());
        assert_eq!(m.open().unwrap().menu.slot_count(), 1);
    }

    #[test]
    fn menu_for_matches_only_the_open_container_id() {
        let mut m = Menus::new();
        m.apply_open_screen(9, 2, "Chest".into());
        assert!(m.menu_for(9).is_some());
        assert!(m.menu_for(8).is_none(), "a different id must not match");
        assert!(
            m.menu_for(0).is_none(),
            "id 0 is the PLAYER's menu and must never resolve to a container"
        );
    }

    #[test]
    fn a_stale_id_after_a_close_finds_nothing() {
        // The failure this prevents: a write addressed to a container that has
        // been closed landing in the next one to open.
        let mut m = Menus::new();
        m.apply_open_screen(9, 2, "Chest".into());
        m.apply_close();
        assert!(m.menu_for(9).is_none());
        m.apply_open_screen(10, 2, "Another".into());
        assert!(m.menu_for(9).is_none(), "the old id must not match the new menu");
        assert!(m.menu_for(10).is_some());
    }

    #[test]
    fn reopening_replaces_the_slots_rather_than_keeping_them() {
        // `MenuScreens.create` builds a fresh menu; a second open_screen for
        // the same id must not inherit the previous container's contents.
        let mut m = Menus::new();
        m.apply_open_screen(3, 2, "Chest".into());
        m.menu_for(3).unwrap().set_content(1, &vec![None; 63], None);
        let before = m.open().unwrap().menu.state_id();
        m.apply_open_screen(3, 2, "Chest".into());
        assert_eq!(m.open().unwrap().menu.state_id(), 0, "state id resets");
        assert_ne!(before, 0, "and the first open really had advanced it");
    }

    // -- M91: the furnace's derived progress -------------------------------

    fn furnace(data: [i16; 4]) -> Menus {
        let mut m = Menus::new();
        m.apply_open_screen(3, 14, "Furnace".into());
        for (i, v) in data.iter().enumerate() {
            assert!(m.apply_set_data(3, i as i16, *v));
        }
        m
    }

    #[test]
    fn a_zero_lit_duration_falls_back_to_200_rather_than_dividing_by_zero() {
        // The fallback is a real substitution, not a guard. Dividing by zero
        // gives infinity, which clamps to 1.0 — a permanently full flame on a
        // furnace that has never been lit.
        let m = furnace([100, 0, 0, 0]);
        let f = m.open().unwrap();
        assert!((f.furnace_lit_progress() - 0.5).abs() < 1e-6, "100 / 200");
        assert!(f.furnace_is_lit());
    }

    #[test]
    fn lit_progress_clamps_and_an_unlit_furnace_reads_zero() {
        let m = furnace([0, 200, 0, 0]);
        assert_eq!(m.open().unwrap().furnace_lit_progress(), 0.0);
        assert!(!m.open().unwrap().furnace_is_lit());
        // More remaining than the duration cannot exceed a full flame.
        let m = furnace([400, 200, 0, 0]);
        assert_eq!(m.open().unwrap().furnace_lit_progress(), 1.0);
    }

    #[test]
    fn burn_progress_is_zero_when_either_field_is_zero() {
        // total == 0 is the one that matters — it is what would divide by
        // zero. current == 0 is redundant (0/total is 0 already) and is
        // transcribed because the source says it.
        assert_eq!(furnace([0, 0, 100, 0]).open().unwrap().furnace_burn_progress(), 0.0);
        assert_eq!(furnace([0, 0, 0, 200]).open().unwrap().furnace_burn_progress(), 0.0);
        let m = furnace([0, 0, 100, 200]);
        assert!((m.open().unwrap().furnace_burn_progress() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_data_indices_are_the_ones_container_set_data_carries() {
        // A transposition here is invisible: the flame would track cooking and
        // the arrow the fuel, both animating plausibly.
        let m = furnace([50, 100, 3, 4]);
        let f = m.open().unwrap();
        assert_eq!(f.data(FURNACE_LIT_REMAINING), 50);
        assert_eq!(f.data(FURNACE_LIT_DURATION), 100);
        assert_eq!(f.data(FURNACE_COOK_PROGRESS), 3);
        assert_eq!(f.data(FURNACE_COOK_TOTAL), 4);
        assert!((f.furnace_lit_progress() - 0.5).abs() < 1e-6);
        assert!((f.furnace_burn_progress() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn close_ignores_the_id_it_was_given() {
        // handleContainerClose compares nothing.
        let mut m = Menus::new();
        m.apply_open_screen(4, 0, "Small".into());
        m.apply_close();
        assert!(m.open().is_none());
    }
}
