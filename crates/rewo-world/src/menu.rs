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

/// `BrewingStandMenu`'s two data slots — **and they are the other way round
/// from the furnace's** (M92).
///
/// `getBrewingTicks()` is `brewingStandData.get(0)` and `getFuel()` is
/// `get(1)`, where `AbstractFurnaceMenu` puts its *fuel* at 0 and its cook
/// progress at 2. Naming these by analogy with the furnace — "0 is the fuel,
/// it was last time" — swaps a 0..20 fuel level with a 0..400 tick counter,
/// and the result is a fuel bar pinned full and bubbles that never move. Both
/// menus are five bytes on the wire and neither says which is which; only the
/// accessor does.
pub const BREW_TICKS: i16 = 0;
pub const BREW_FUEL: i16 = 1;

impl OpenMenu {
    /// `getBrewingTicks()` — ticks **remaining**, counting down from 400.
    pub fn brewing_ticks(&self) -> i32 {
        self.data(BREW_TICKS) as i32
    }

    /// `getFuel()` — blaze-powder charges left, 0..=20.
    pub fn brewing_fuel(&self) -> i32 {
        self.data(BREW_FUEL) as i32
    }
}

/// `EnchantmentMenu`'s ten data slots — the largest count in the registry, and
/// what fixes [`MAX_DATA_SLOTS`] (M92).
///
/// ```text
/// 0..=2  costs[i]       the level price of each offer, 0 for "no offer"
/// 3      enchantmentSeed
/// 4..=6  enchantClue[i] an ENCHANTMENT REGISTRY ID, or -1
/// 7..=9  levelClue[i]   the offered level, or -1
/// ```
///
/// **The clue sentinel is `-1`, and 0 is a perfectly valid registry id**, so
/// the two cannot be conflated: a client that treated an absent clue as 0
/// would name whichever enchantment happens to sit at index 0 in the server's
/// registry. That is the reason M87's decode reads a **signed** short, and it
/// is the only place in the container arc where a negative data value is the
/// normal case rather than an edge one.
///
/// The initial values are never observed as Rewo's zeros: `sendAllDataToRemote`
/// hands every data slot to `sendInitialData`, which broadcasts all ten before
/// the screen can draw.
pub const ENCHANT_COST: i16 = 0;
pub const ENCHANT_SEED: i16 = 3;
pub const ENCHANT_CLUE: i16 = 4;
pub const ENCHANT_LEVEL_CLUE: i16 = 7;

/// The menu slot the lapis sits in. Its **count**, not its presence, is what
/// `getGoldCount()` returns.
pub const ENCHANT_LAPIS_SLOT: usize = 1;

impl OpenMenu {
    /// `menu.costs` — the three offers' level prices, 0 meaning "no offer".
    pub fn enchant_costs(&self) -> [i32; 3] {
        std::array::from_fn(|i| self.data(ENCHANT_COST + i as i16) as i32)
    }

    /// `getEnchantmentSeed()`, which seeds the Standard Galactic name.
    pub fn enchant_seed(&self) -> i32 {
        self.data(ENCHANT_SEED) as i32
    }

    /// `menu.enchantClue[i]` — an enchantment registry id, or `None` for the
    /// `-1` sentinel.
    pub fn enchant_clue(&self, i: usize) -> Option<i32> {
        let v = self.data(ENCHANT_CLUE + i as i16) as i32;
        (v >= 0).then_some(v)
    }

    /// `menu.levelClue[i]` — the offered level, or `None` for `-1`.
    pub fn enchant_level_clue(&self, i: usize) -> Option<i32> {
        let v = self.data(ENCHANT_LEVEL_CLUE + i as i16) as i32;
        (v >= 0).then_some(v)
    }

    /// `getGoldCount()` — **the count of the stack in menu slot 1**, not a
    /// data slot. So the lapis half of the affordability test arrives through
    /// `container_set_content`, on a different packet from the costs.
    pub fn enchant_lapis(&self) -> i32 {
        self.menu
            .menu_slot(ENCHANT_LAPIS_SLOT)
            .map_or(0, |s| s.count as i32)
    }

    // -- CrafterMenu (M93h) ------------------------------------------------
    //
    // `containerData` is a `SimpleContainerData(10)`: slots 0..=8 are the
    // per-grid-slot toggle and slot 9 is redstone power. Both live in the same
    // array, which is why `isSlotDisabled` carries its own range guard.

    /// `CrafterMenu.isSlotDisabled` —
    /// `slotId > -1 && slotId < 9 ? containerData.get(slotId) == 1 : false`.
    ///
    /// **1 is DISABLED, not enabled.** The inversion is easy to lose because
    /// `setSlotState` takes an `isEnabled` and writes `isEnabled ? 0 : 1`, so
    /// the argument and the stored value are opposites. Reading it the other
    /// way disables exactly the eight slots the player left on.
    ///
    /// The `< 9` is load-bearing rather than defensive: index **9 is the
    /// power flag**, in the same array, and a crafter that is powered would
    /// otherwise read as having a ninth disabled slot.
    pub fn crafter_slot_disabled(&self, slot: i32) -> bool {
        (0..CRAFTER_GRID_SLOTS).contains(&slot) && self.data(slot as i16) == 1
    }

    /// Whether this menu **replaces** a slot's normal render, so its item must
    /// not be drawn (M93j).
    ///
    /// `CrafterScreen.extractSlot` calls `extractDisabledSlot` *instead of*
    /// `super.extractSlot`, so a disabled crafter slot shows the cover and no
    /// item. Note this is the opposite composition from the toggle itself,
    /// which is **additive** (M93i): the two halves of one feature compose the
    /// two different ways, and swapping them either hides an enabled slot's
    /// item or paints the cover underneath one.
    ///
    /// A general predicate rather than a crafter check inlined in the icon
    /// path, because "this screen draws something else here" is the shape any
    /// future `extractSlot` override takes.
    pub fn slot_hides_item(&self, slot: usize) -> bool {
        i32::try_from(slot).is_ok_and(|s| self.crafter_slot_disabled(s))
    }

    /// `CrafterMenu.isPowered` — `containerData.get(9) == 1`.
    pub fn crafter_powered(&self) -> bool {
        self.data(CRAFTER_POWERED_DATA_SLOT) == 1
    }
}

/// `CrafterMenu`'s grid size, and the exclusive bound `isSlotDisabled` tests.
pub const CRAFTER_GRID_SLOTS: i32 = 9;
/// The data slot holding redstone power — **inside the same array** as the
/// nine toggles, which is why the toggle accessor bounds itself.
pub const CRAFTER_POWERED_DATA_SLOT: i16 = 9;

/// What clicking a crafter's grid slot does, before the ordinary click runs.
///
/// `CrafterScreen.slotClicked` gates on **three** things —
/// `slot instanceof CrafterSlot && !slot.hasItem() && !player.isSpectator()`
/// — so an occupied slot cannot be toggled at all, and then dispatches:
///
/// ```java
/// case PICKUP:
///    if (menu.isSlotDisabled(slotId))      enableSlot(slotId);
///    else if (menu.getCarried().isEmpty()) disableSlot(slotId);
///    break;
/// case SWAP:
///    if (menu.isSlotDisabled(slotId) && !playerInventoryItem.isEmpty())
///       enableSlot(slotId);
/// ```
///
/// **The PICKUP branch is asymmetric.** Re-enabling a disabled slot is
/// unconditional; disabling an enabled one requires an EMPTY cursor, because
/// clicking an empty enabled slot while holding something is a placement. A
/// symmetric reading would make it impossible to put an item into a crafter by
/// hand — it would toggle the slot off instead.
///
/// **And whatever this returns, the ordinary click still happens**:
/// `slotClicked` ends in `super.slotClicked(...)` unconditionally, so a toggle
/// is *additive* rather than a replacement. Treating it as an either/or drops
/// the placement that the SWAP case exists to complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrafterToggle {
    /// Send `container_slot_state_changed` with `newState = true`.
    Enable,
    /// ...with `newState = false`.
    Disable,
    /// No toggle — the ordinary click still runs.
    None,
}

/// Whether a click lands on a crafter's grid, which is the outer gate before
/// any of the toggle logic (M93i).
///
/// Extracted rather than left inline in `PlaySession::crafter_slot_click`
/// because **`PlaySession` has no test module anywhere in the repo** — it owns
/// a socket — which is the hazard M71 recorded when its whole packet fan-out
/// turned out to be unwitnessed. Everything the adapter does except the send
/// itself lives in a tested function for that reason.
pub fn is_crafter_grid_slot(menu_protocol_id: i32, slot: i32) -> bool {
    menu_protocol_id == CRAFTER_MENU_PROTOCOL_ID && (0..CRAFTER_GRID_SLOTS).contains(&slot)
}

/// `minecraft:menu`'s `crafter_3x3` id.
pub const CRAFTER_MENU_PROTOCOL_ID: i32 = 7;

impl OpenMenu {
    /// `CrafterMenu.setSlotState(slotId, isEnabled)` — the local half of a
    /// toggle, applied before/alongside the packet (M93i).
    ///
    /// **Stores the inverse of its argument** (`isEnabled ? 0 : 1`), which is
    /// the same inversion [`Self::crafter_slot_disabled`] reads back. Writing
    /// the argument straight through makes the next click on the same slot see
    /// the opposite state and toggle it back.
    pub fn set_crafter_slot_state(&mut self, slot: i32, enabled: bool) -> bool {
        if !(0..CRAFTER_GRID_SLOTS).contains(&slot) {
            return false;
        }
        match self.data.get_mut(slot as usize) {
            Some(v) => {
                *v = i16::from(!enabled);
                true
            }
            None => false,
        }
    }
}

/// `CrafterScreen.slotClicked`'s toggle decision (M93h, corrected in M93i).
///
/// `swap_target_empty` is `player.getInventory().getItem(buttonNum).isEmpty()`
/// and is only consulted on the SWAP path.
///
/// # Why this takes the raw input and not an `is_swap` flag
///
/// M93h took `is_swap: bool` and so treated **every** non-swap input as
/// PICKUP — including QUICK_MOVE, THROW, PICKUP_ALL and QUICK_CRAFT. Vanilla's
/// `switch (containerInput)` has `case PICKUP` and `case SWAP` and **no
/// default**, so those four fall straight through and toggle nothing. The bug
/// was invisible while the function had no caller: shift-clicking a disabled
/// crafter slot would have silently re-enabled it. Wiring it up is what
/// surfaced that, which is the argument for not leaving a model unwired for
/// long.
pub fn crafter_toggle(
    input: i32,
    disabled: bool,
    slot_occupied: bool,
    spectator: bool,
    carried_empty: bool,
    swap_target_empty: bool,
) -> CrafterToggle {
    use crate::inventory::{CONTAINER_INPUT_PICKUP, CONTAINER_INPUT_SWAP};
    if slot_occupied || spectator {
        return CrafterToggle::None;
    }
    match input {
        CONTAINER_INPUT_PICKUP => {
            if disabled {
                CrafterToggle::Enable
            } else if carried_empty {
                CrafterToggle::Disable
            } else {
                CrafterToggle::None
            }
        }
        // SWAP has no disable arm at all: a swap can only turn a slot back on,
        // and only when it has something to put there.
        CONTAINER_INPUT_SWAP if disabled && !swap_target_empty => CrafterToggle::Enable,
        // Every other input — the `switch` has no default.
        _ => CrafterToggle::None,
    }
}

/// `BeaconMenu`'s three data slots (M92).
///
/// ```text
/// 0  levels     the pyramid height, 0..=4
/// 1  primary    encodeEffect(primary)
/// 2  secondary  encodeEffect(secondary)
/// ```
///
/// # Two adjacent screens, two different "absent" encodings
///
/// ```java
/// encodeEffect(e) = e == null ? 0 : id(e) + 1;
/// decodeEffect(v) = v == 0 ? null : byId(v - 1);
/// ```
///
/// So the beacon says "no effect" with **0 and shifts every real id up by
/// one**, where the enchanting table one menu earlier says "no clue" with
/// **-1 and leaves its ids alone**. Both travel in the same signed short on
/// the same packet, and neither is inferable from the wire. Carrying one
/// convention across is a silent off-by-one: every beacon effect would render
/// as its registry neighbour, and speed — id 0, encoded 1 — would read as the
/// effect whose id is 1.
///
/// It is `holder`'s `id + 1` scheme (M16/M21/M55's recurring fork) turning up
/// inside a **data slot**, where every other value in the arc is raw.
pub const BEACON_LEVELS: i16 = 0;
pub const BEACON_PRIMARY: i16 = 1;
pub const BEACON_SECONDARY: i16 = 2;

/// `BeaconMenu.decodeEffect` — `0` is absent, anything else is `id - 1`.
pub fn decode_beacon_effect(v: i16) -> Option<i32> {
    (v != 0).then(|| v as i32 - 1)
}

/// `BeaconMenu.encodeEffect` — the inverse, kept beside it so the pair cannot
/// drift.
pub fn encode_beacon_effect(id: Option<i32>) -> i16 {
    id.map_or(0, |id| (id + 1) as i16)
}

impl OpenMenu {
    /// `getLevels()` — the pyramid's height.
    pub fn beacon_levels(&self) -> i32 {
        self.data(BEACON_LEVELS) as i32
    }

    /// `getPrimaryEffect()` as a `minecraft:mob_effect` registry id.
    pub fn beacon_primary(&self) -> Option<i32> {
        decode_beacon_effect(self.data(BEACON_PRIMARY))
    }

    /// `getSecondaryEffect()`.
    pub fn beacon_secondary(&self) -> Option<i32> {
        decode_beacon_effect(self.data(BEACON_SECONDARY))
    }

    /// `menu.hasPayment()` — whether the payment slot (menu slot 0) holds
    /// anything. Like the enchanting table's lapis, this is a **slot**, so it
    /// arrives on a different packet from the levels.
    pub fn beacon_has_payment(&self) -> bool {
        self.menu.menu_slot(0).is_some()
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

    // -- M92: the brewing stand's data slots, which invert ------------------

    #[test]
    fn the_brewing_stands_data_slots_are_the_reverse_of_the_furnaces() {
        // The whole point: slot 0 is the TICK COUNTER here and the FUEL in a
        // furnace. Transposing them is invisible on the wire — both menus send
        // the same five bytes — and shows up only as a fuel bar pinned full
        // and bubbles that never move.
        let mut m = Menus::new();
        m.apply_open_screen(3, 11, "Brewing Stand".into()); // brewing_stand
        assert!(m.apply_set_data(3, 0, 380), "slot 0 is the tick counter");
        assert!(m.apply_set_data(3, 1, 17), "slot 1 is the fuel");
        let b = m.open().unwrap();
        assert_eq!(b.brewing_ticks(), 380);
        assert_eq!(b.brewing_fuel(), 17);
        // A furnace's slot 0 is the fuel, and the two accessors must not be
        // reading the same constant.
        assert_ne!(BREW_TICKS, FURNACE_LIT_DURATION);
        assert_eq!(BREW_TICKS, FURNACE_LIT_REMAINING, "the same index, the other meaning");
    }

    #[test]
    fn an_untouched_brewing_stand_reads_zero_on_both() {
        let mut m = Menus::new();
        m.apply_open_screen(1, 11, "Brewing Stand".into());
        let b = m.open().unwrap();
        assert_eq!((b.brewing_ticks(), b.brewing_fuel()), (0, 0));
    }

    // -- M92: the enchanting table's ten data slots -------------------------

    fn enchant(data: &[(i16, i16)]) -> Menus {
        let mut m = Menus::new();
        m.apply_open_screen(6, 13, "Enchant".into()); // enchantment
        for &(id, v) in data {
            assert!(m.apply_set_data(6, id, v), "slot {id}");
        }
        m
    }

    #[test]
    fn the_ten_enchantment_slots_land_in_their_three_groups() {
        // A transposition here is invisible: every one of these is a small
        // non-negative integer, so costs read as clues and clues as levels
        // all render something plausible.
        let m = enchant(&[
            (0, 5), (1, 12), (2, 30),      // costs
            (3, 424242i32 as i16),         // seed
            (4, 7), (5, 8), (6, 9),        // enchant clues
            (7, 1), (8, 2), (9, 3),        // level clues
        ]);
        let e = m.open().unwrap();
        assert_eq!(e.enchant_costs(), [5, 12, 30]);
        assert_eq!(e.enchant_clue(0), Some(7));
        assert_eq!(e.enchant_clue(2), Some(9));
        assert_eq!(e.enchant_level_clue(0), Some(1));
        assert_eq!(e.enchant_level_clue(2), Some(3));
        assert_eq!(e.enchant_seed(), 424242i32 as i16 as i32);
    }

    #[test]
    fn minus_one_is_no_clue_and_zero_is_enchantment_number_zero() {
        // The sentinel is -1 and 0 is a valid registry id, so the two must not
        // collapse — a client conflating them names whichever enchantment sits
        // at index 0 in the server's registry. This is also why the wire field
        // is a SIGNED short (M87).
        let m = enchant(&[(4, -1), (5, 0), (7, -1), (8, 0)]);
        let e = m.open().unwrap();
        assert_eq!(e.enchant_clue(0), None, "-1 is absent");
        assert_eq!(e.enchant_clue(1), Some(0), "0 is present, and is id 0");
        assert_eq!(e.enchant_level_clue(0), None);
        assert_eq!(e.enchant_level_clue(1), Some(0));
    }

    #[test]
    fn the_lapis_count_comes_from_a_menu_slot_not_a_data_slot() {
        // `getGoldCount()` reads the COUNT of the stack in menu slot 1, so the
        // affordability test's two halves arrive on two different packets.
        let mut m = enchant(&[(0, 5)]);
        assert_eq!(m.open().unwrap().enchant_lapis(), 0, "an empty slot is zero");
        let inv = m.menu_for(6).unwrap();
        let mut content = vec![None; inv.slot_count()];
        content[ENCHANT_LAPIS_SLOT] = Some(crate::inventory::ItemSlot {
            item_id: 1,
            count: 13,
            has_components: false,
            components: 0,
            damage: None,
            max_damage: None,
            enchanted: false,
            any_enchantments: false,
            unbreakable: false,
            damage_component_removed: false,
            has_map_id: false,
            dye_removed: false,
            provides_banner_patterns_removed: false,
            trim_material: None,
        });
        inv.set_content(1, &content, None);
        assert_eq!(m.open().unwrap().enchant_lapis(), 13, "the COUNT, not 1");
    }

    #[test]
    fn ten_data_slots_is_the_registry_maximum() {
        // MAX_DATA_SLOTS exists because of this menu; if it ever shrank, the
        // level clues would silently stop arriving.
        assert_eq!(MAX_DATA_SLOTS, 10);
        assert_eq!(ENCHANT_LEVEL_CLUE as usize + 2, MAX_DATA_SLOTS - 1);
    }

    // -- M92: the beacon's data slots ---------------------------------------

    #[test]
    fn the_beacon_says_absent_with_zero_where_the_enchanting_table_says_minus_one() {
        // Two adjacent menus, two conventions, one signed short. Carrying
        // either across is a silent off-by-one: with the enchanting table's
        // rule a beacon's 0 would read as effect id 0 (speed) and every real
        // effect as its neighbour.
        assert_eq!(decode_beacon_effect(0), None, "0 is ABSENT here");
        assert_eq!(decode_beacon_effect(1), Some(0), "and 1 is id 0");
        assert_eq!(decode_beacon_effect(11), Some(10), "resistance");
        // The enchanting table's -1/0 pair, for contrast.
        let e = {
            let mut m = Menus::new();
            m.apply_open_screen(1, 13, "E".into());
            m.apply_set_data(1, ENCHANT_CLUE, 0);
            m
        };
        assert_eq!(
            e.open().unwrap().enchant_clue(0),
            Some(0),
            "0 is PRESENT there — the opposite reading of the same byte"
        );
    }

    #[test]
    fn encode_and_decode_beacon_effects_round_trip() {
        for id in [None, Some(0), Some(1), Some(10), Some(31)] {
            assert_eq!(decode_beacon_effect(encode_beacon_effect(id)), id, "{id:?}");
        }
    }

    #[test]
    fn the_beacons_three_slots_land_where_they_belong() {
        let mut m = Menus::new();
        m.apply_open_screen(2, 9, "Beacon".into()); // beacon
        for (id, v) in [(0i16, 3i16), (1, 5), (2, 0)] {
            assert!(m.apply_set_data(2, id, v));
        }
        let b = m.open().unwrap();
        assert_eq!(b.beacon_levels(), 3);
        assert_eq!(b.beacon_primary(), Some(4), "encoded 5 is id 4");
        assert_eq!(b.beacon_secondary(), None, "0 is no secondary");
        assert!(!b.beacon_has_payment(), "the payment slot is a SLOT, not data");
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

#[cfg(test)]
mod m93h_crafter {
    use super::*;
    use crate::inventory::{CONTAINER_INPUT_PICKUP as PICKUP, CONTAINER_INPUT_SWAP as SWAP};

    /// A crafter with the given ten data values.
    fn crafter(data: [i16; 10]) -> Menus {
        let mut m = Menus::new();
        m.apply_open_screen(4, 7, "Crafter".into());
        for (i, v) in data.iter().enumerate() {
            assert!(m.apply_set_data(4, i as i16, *v));
        }
        m
    }

    #[test]
    fn one_is_disabled_and_zero_is_enabled() {
        // THE inversion. `setSlotState` takes an `isEnabled` and stores
        // `isEnabled ? 0 : 1`, so the argument and the stored value are
        // opposites — reading the value as "enabled" disables exactly the
        // slots the player left on, which looks like a working crafter that
        // never crafts.
        let mut d = [0i16; 10];
        d[3] = 1;
        let m = crafter(d);
        let o = m.open().unwrap();
        assert!(o.crafter_slot_disabled(3), "1 means DISABLED");
        for s in [0, 1, 2, 4, 5, 6, 7, 8] {
            assert!(!o.crafter_slot_disabled(s), "0 means enabled, slot {s}");
        }
    }

    #[test]
    fn the_power_flag_shares_the_array_and_is_not_a_tenth_slot() {
        // `isSlotDisabled` guards `slotId < 9` and index 9 is the power flag,
        // in the SAME array. Without the guard a powered crafter reads as
        // having a ninth disabled slot — and 9 is a legal index, so nothing
        // would fault.
        let mut d = [0i16; 10];
        d[9] = 1;
        let m = crafter(d);
        let o = m.open().unwrap();
        assert!(o.crafter_powered());
        assert!(!o.crafter_slot_disabled(9), "index 9 is power, not a slot");
        // ...and a negative or oversized index is not a slot either.
        assert!(!o.crafter_slot_disabled(-1));
        assert!(!o.crafter_slot_disabled(9));
    }

    #[test]
    fn the_pickup_branch_is_asymmetric() {
        // Re-enabling is unconditional; DISABLING needs an empty cursor,
        // because clicking an empty enabled slot while holding something is a
        // placement. A symmetric reading makes it impossible to put an item
        // into a crafter by hand — it toggles the slot off instead.
        let enable = crafter_toggle(PICKUP, true, false, false, true, false);
        assert_eq!(enable, CrafterToggle::Enable);
        // ...still enable even with a full cursor.
        assert_eq!(
            crafter_toggle(PICKUP, true, false, false, false, false),
            CrafterToggle::Enable
        );
        // Enabled + empty cursor -> disable.
        assert_eq!(
            crafter_toggle(PICKUP, false, false, false, true, false),
            CrafterToggle::Disable
        );
        // Enabled + FULL cursor -> nothing, so the placement can happen.
        assert_eq!(
            crafter_toggle(PICKUP, false, false, false, false, false),
            CrafterToggle::None
        );
    }

    #[test]
    fn swap_can_only_turn_a_slot_back_on() {
        // The SWAP arm has no disable branch at all, and it needs something to
        // put there.
        assert_eq!(
            crafter_toggle(SWAP, true, false, false, true, false),
            CrafterToggle::Enable
        );
        assert_eq!(
            crafter_toggle(SWAP, true, false, false, true, true),
            CrafterToggle::None,
            "an empty swap target enables nothing"
        );
        assert_eq!(
            crafter_toggle(SWAP, false, false, false, true, false),
            CrafterToggle::None,
            "SWAP never disables"
        );
    }

    #[test]
    fn the_grid_gate_admits_only_a_crafters_nine_slots() {
        // The outer gate, extracted so it has a witness at all: it lives in
        // `PlaySession::crafter_slot_click`, and PlaySession has no test
        // module anywhere in the repo (M71).
        for slot in 0..9 {
            assert!(is_crafter_grid_slot(CRAFTER_MENU_PROTOCOL_ID, slot));
        }
        // Slot 9 is the crafter's first PLAYER slot, and 45 is its result.
        assert!(!is_crafter_grid_slot(CRAFTER_MENU_PROTOCOL_ID, 9));
        assert!(!is_crafter_grid_slot(CRAFTER_MENU_PROTOCOL_ID, 45));
        assert!(!is_crafter_grid_slot(CRAFTER_MENU_PROTOCOL_ID, -1));
        // ...and no other menu has toggles, however like a crafter it looks.
        // 12 is `crafting`, which also has a 3x3 grid at slots 1..=9.
        for other in [0, 2, 12, 14, 18, 23] {
            assert!(!is_crafter_grid_slot(other, 0), "menu {other} has no toggles");
        }
    }

    #[test]
    fn the_local_apply_stores_the_inverse_of_its_argument() {
        // `setSlotState` takes an `isEnabled` and stores `isEnabled ? 0 : 1`.
        // Writing the argument straight through makes the NEXT click on the
        // same slot see the opposite state and toggle it back — a crafter slot
        // that refuses to stay off.
        let mut m = crafter([0i16; 10]);
        let o = m.open_mut().unwrap();
        assert!(o.set_crafter_slot_state(4, false));
        assert_eq!(o.data(4), 1, "disabling stores 1");
        assert!(o.crafter_slot_disabled(4), "and reads back as disabled");
        assert!(o.set_crafter_slot_state(4, true));
        assert_eq!(o.data(4), 0);
        assert!(!o.crafter_slot_disabled(4));
        // Out of range writes nothing — and 9 is the POWER flag, which a
        // toggle must never clobber.
        assert!(!o.set_crafter_slot_state(9, false));
        assert_eq!(o.data(9), 0, "the power flag is untouched");
        assert!(!o.set_crafter_slot_state(-1, false));
    }

    #[test]
    fn every_input_but_pickup_and_swap_toggles_nothing() {
        // The defect M93h shipped and wiring it up exposed. `switch
        // (containerInput)` has `case PICKUP` and `case SWAP` and NO DEFAULT,
        // so the other four fall straight through. The old signature took an
        // `is_swap: bool` and so ran the PICKUP arm for all of them —
        // shift-clicking a disabled crafter slot would have silently
        // re-enabled it, and no witness could see it because the function had
        // no caller.
        use crate::inventory::{
            CONTAINER_INPUT_PICKUP_ALL, CONTAINER_INPUT_QUICK_CRAFT, CONTAINER_INPUT_QUICK_MOVE,
            CONTAINER_INPUT_THROW,
        };
        for input in [
            CONTAINER_INPUT_QUICK_MOVE,
            CONTAINER_INPUT_THROW,
            CONTAINER_INPUT_PICKUP_ALL,
            CONTAINER_INPUT_QUICK_CRAFT,
        ] {
            // The exact state that WOULD toggle under PICKUP or SWAP.
            assert_eq!(
                crafter_toggle(input, true, false, false, true, false),
                CrafterToggle::None,
                "input {input} must not toggle"
            );
            assert_eq!(
                crafter_toggle(input, false, false, false, true, false),
                CrafterToggle::None,
                "input {input} must not disable either"
            );
        }
        // ...and the two that DO, so this is not vacuous.
        assert_eq!(
            crafter_toggle(PICKUP, true, false, false, true, false),
            CrafterToggle::Enable
        );
        assert_eq!(
            crafter_toggle(SWAP, true, false, false, true, false),
            CrafterToggle::Enable
        );
    }

    #[test]
    fn an_occupied_slot_or_a_spectator_toggles_nothing() {
        // Both are outer gates, before the input dispatch.
        for input in [PICKUP, SWAP] {
            assert_eq!(
                crafter_toggle(input, true, true, false, true, false),
                CrafterToggle::None,
                "an occupied slot cannot toggle (input={input})"
            );
            assert_eq!(
                crafter_toggle(input, true, false, true, true, false),
                CrafterToggle::None,
                "a spectator cannot toggle (input={input})"
            );
        }
    }
}

#[cfg(test)]
mod m93k_tooltip {
    use super::*;
    use crate::inventory::CONTAINER_INPUT_PICKUP as PICKUP;

    /// M93k — the `gui.togglable_slot` hint's condition IS the click's.
    ///
    /// Vanilla writes them as two separate expressions:
    ///
    /// ```java
    /// // the hint
    /// hoveredSlot instanceof CrafterSlot && !isSlotDisabled(index)
    ///   && getCarried().isEmpty() && !hoveredSlot.hasItem() && !isSpectator()
    /// // the click
    /// case PICKUP: if (isSlotDisabled) enable; else if (getCarried().isEmpty()) disable;
    /// ```
    ///
    /// and they agree exactly: the hint shows iff a plain click would DISABLE
    /// the slot. Deriving it means the tooltip cannot promise an action the
    /// click will not take — which is what "Click to disable slot" claims.
    #[test]
    fn the_hint_shows_exactly_when_a_click_would_disable_the_slot() {
        for disabled in [false, true] {
            for occupied in [false, true] {
                for spectator in [false, true] {
                    for carried_empty in [false, true] {
                        let vanilla_hint =
                            !disabled && carried_empty && !occupied && !spectator;
                        let would_disable = crafter_toggle(
                            PICKUP,
                            disabled,
                            occupied,
                            spectator,
                            carried_empty,
                            false,
                        ) == CrafterToggle::Disable;
                        assert_eq!(
                            vanilla_hint, would_disable,
                            "disabled={disabled} occupied={occupied} \
                             spectator={spectator} carried_empty={carried_empty}"
                        );
                    }
                }
            }
        }
    }

    /// ...and the equivalence is not vacuous: both are true somewhere and
    /// false somewhere, so the test above is not comparing two constants.
    #[test]
    fn the_hint_is_neither_always_on_nor_always_off() {
        assert_eq!(
            crafter_toggle(PICKUP, false, false, false, true, false),
            CrafterToggle::Disable,
            "an empty enabled slot with an empty cursor shows the hint"
        );
        for (d, o, sp, ce) in [
            (true, false, false, true),   // already disabled
            (false, true, false, true),   // has an item
            (false, false, true, true),   // spectator
            (false, false, false, false), // holding something
        ] {
            assert_ne!(
                crafter_toggle(PICKUP, d, o, sp, ce, false),
                CrafterToggle::Disable,
                "disabled={d} occupied={o} spectator={sp} carried_empty={ce}"
            );
        }
    }
}
