//! The slot layout of every `minecraft:menu` type.
//!
//! Vanilla builds a menu's slot list imperatively in each `*Menu` constructor:
//! a few `addSlot(new Slot(container, index, x, y))` calls, some loops, and
//! `addStandardInventorySlots(inventory, left, top)` for the player's 36. The
//! **order of those calls is the menu slot index**, which is the index the wire
//! speaks (`container_set_slot`, and the position within
//! `container_set_content`'s list). So this table is a transcription of the
//! *call sequence*, not of a set of coordinates.
//!
//! # Why this is a hand-written table and not a generator
//!
//! Every other bulk fact in Rewo comes from a `tools/gen_*.py` reading the
//! decompile, and that was tried first here. It does not work, and the reason
//! is measurable rather than aesthetic: the 25 menus define their slots in
//! **four different idioms** — a direct `addSlot(new Slot(c, i, x, y))`, a
//! nested loop, a field assigned earlier (`BeaconMenu.paymentSlot`), and a
//! fluent builder consumed by a *base class* (`ItemCombinerMenuSlotDefinition`,
//! for anvil and smithing) — plus five menus that declare no slots at all and
//! inherit them (`furnace`/`blast_furnace`/`smoker` from `AbstractFurnaceMenu`,
//! `anvil`/`smithing` from `ItemCombinerMenu`, `crafting` from
//! `AbstractCraftingMenu`). A regex pass recovers **17 of 25**; chasing the
//! rest across class boundaries is a small Java interpreter, and its failure
//! mode is the one that must not happen — a silently *short* slot list, which
//! shifts every later index and looks like a plausible menu.
//!
//! So the positions are transcribed here by hand and the honesty comes from the
//! gate instead, on the M16 `dimensioncheck` pattern: an independent
//! re-derivation grades this table rather than importing it.
//!
//! # The one that inverts
//!
//! **`crafter_3x3` puts its result slot *after* the player inventory.**
//! `CrafterMenu.addSlots` is grid (0..8), then `addStandardInventorySlots`
//! (9..44), then `addSlot(NonInteractiveResultSlot)` at **45**. Every other
//! menu in the registry appends the player's 36 last. An implementation that
//! assumes "container slots, then the player's" puts the crafter's output
//! inside the player's inventory and shifts nothing else, so it reads as a
//! one-slot bug rather than a layout bug.
//!
//! # The one with no player inventory
//!
//! **`lectern` has exactly one slot and never calls
//! `addStandardInventorySlots`.** `LecternMenu` adds the book at `(0, 0)` — a
//! real position, not a sentinel; `LecternScreen` simply does not draw it. Any
//! code that assumes a menu ends with 36 player slots is wrong here, and a
//! `slots.len() - 36` is a panic.

/// One `addSlot` call, or one loop of them, in the order vanilla makes them.
///
/// The pitch is 18 px everywhere; vanilla writes `left + x * 18` in every grid
/// it builds, so it is not a per-block parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotBlock {
    /// A single `addSlot(new Slot(_, _, x, y))`.
    One { x: i16, y: i16 },
    /// A `cols` x `rows` loop laid out row-major from `(left, top)`.
    ///
    /// Row-major because every vanilla grid loop is `for y { for x { } }` —
    /// column-major would transpose a 9x3 chest into something that still has
    /// 27 slots and still looks like a chest.
    Grid {
        left: i16,
        top: i16,
        cols: u8,
        rows: u8,
    },
    /// `AbstractContainerMenu.addStandardInventorySlots(inventory, left, top)`.
    ///
    /// Expands to the 3x9 "extended" block at `(left, top)` — inventory indices
    /// `9..=35`, i.e. `x + (y + 1) * 9` — followed by the 9 hotbar slots
    /// (indices `0..=8`) at `top + 58`. **The hotbar's offset is a named
    /// constant `topToHotbar = 58`, not `3 * 18 = 54`**: there is a 4 px gap
    /// (`hotbarSeparator`), which is why a naive `top + rows * 18` puts the
    /// hotbar 4 px high and overlapping the row above it.
    StandardInventory { left: i16, top: i16 },
}

/// The player inventory's own menu (`container_id` 0) is not in the
/// `minecraft:menu` registry, so it has no protocol id.
pub const NO_PROTOCOL_ID: i32 = -1;

/// Vertical distance from an `addStandardInventorySlots` origin to the hotbar.
pub const TOP_TO_HOTBAR: i16 = 58;

/// The 18 px pitch every vanilla slot grid uses.
pub const SLOT_PITCH: i16 = 18;

/// A menu type: its wire id, its slot construction order, and the geometry its
/// screen is drawn at.
#[derive(Debug, Clone, Copy)]
pub struct MenuLayout {
    /// The `minecraft:menu` registry id. This is what `open_screen` carries,
    /// as a **raw 0-based VarInt** — `ByteBufCodecs.registry(Registries.MENU)`,
    /// not `holder`'s `id + 1`.
    pub protocol_id: i32,
    /// The registry name, without the `minecraft:` namespace.
    pub name: &'static str,
    /// `addSlot` calls in vanilla's order. Index into the expansion of this is
    /// the menu slot index the wire speaks.
    pub blocks: &'static [SlotBlock],
    /// `AbstractContainerScreen.imageWidth` — 176 unless the screen overrides.
    pub image_w: i32,
    /// `AbstractContainerScreen.imageHeight`.
    pub image_h: i32,
}

impl MenuLayout {
    /// Total slot count — the length of the wire's `container_set_content` list.
    pub const fn slot_count(&self) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i < self.blocks.len() {
            n += match self.blocks[i] {
                SlotBlock::One { .. } => 1,
                SlotBlock::Grid { cols, rows, .. } => (cols as usize) * (rows as usize),
                SlotBlock::StandardInventory { .. } => 36,
            };
            i += 1;
        }
        n
    }

    /// Where one menu slot's 16x16 icon sits, relative to the GUI's top-left.
    ///
    /// Walks the blocks rather than expanding them, because the hover test
    /// asks this for every slot of the menu on every frame — `getHoveredSlot`
    /// is a linear scan, so an allocating accessor would allocate once per
    /// slot per frame.
    pub fn position(&self, slot: usize) -> Option<(i16, i16)> {
        let mut base = 0usize;
        for block in self.blocks {
            let (len, pos) = match *block {
                SlotBlock::One { x, y } => (1usize, Some((x, y))),
                SlotBlock::Grid {
                    left,
                    top,
                    cols,
                    rows,
                } => {
                    let (cols, rows) = (cols as usize, rows as usize);
                    let len = cols * rows;
                    let i = slot.wrapping_sub(base);
                    (
                        len,
                        (i < len).then(|| {
                            (
                                left + (i % cols) as i16 * SLOT_PITCH,
                                top + (i / cols) as i16 * SLOT_PITCH,
                            )
                        }),
                    )
                }
                SlotBlock::StandardInventory { left, top } => {
                    let i = slot.wrapping_sub(base);
                    (
                        36usize,
                        (i < 36).then(|| {
                            if i < 27 {
                                (
                                    left + (i % 9) as i16 * SLOT_PITCH,
                                    top + (i / 9) as i16 * SLOT_PITCH,
                                )
                            } else {
                                (left + (i - 27) as i16 * SLOT_PITCH, top + TOP_TO_HOTBAR)
                            }
                        }),
                    )
                }
            };
            if slot >= base && slot < base + len {
                return pos;
            }
            base += len;
        }
        None
    }

    /// `AbstractContainerScreen.isHovering` — **an 18x18 box, not 16x16**.
    ///
    /// The icon is 16 px, but the test is `x >= left - 1 && x < left + w + 1`
    /// with `w = 16`, so it reaches one pixel out on every side and the slots
    /// tile without gaps. Using the icon's own rect leaves a one-pixel dead
    /// cross between every pair of neighbours.
    pub fn slot_contains(&self, slot: usize, gui_x: f64, gui_y: f64) -> bool {
        let Some((left, top)) = self.position(slot) else {
            return false;
        };
        let (left, top) = (left as f64, top as f64);
        gui_x >= left - 1.0 && gui_x < left + 17.0 && gui_y >= top - 1.0 && gui_y < top + 17.0
    }

    /// The menu slot under a GUI-relative point, or `None`.
    ///
    /// The boxes overlap by their one-pixel bleed and vanilla's
    /// `getHoveredSlot` returns the **first** match in menu order, so this
    /// scans rather than computing an index — and the scan order is the menu's
    /// slot order, which for `crafter_3x3` means the result slot is tested
    /// last because it *is* last.
    pub fn slot_at(&self, gui_x: f64, gui_y: f64) -> Option<usize> {
        (0..self.slot_count()).find(|&s| self.slot_contains(s, gui_x, gui_y))
    }

    /// Expand the blocks into one position per menu slot index, in wire order.
    pub fn positions(&self) -> Vec<(i16, i16)> {
        let mut out = Vec::with_capacity(self.slot_count());
        for block in self.blocks {
            match *block {
                SlotBlock::One { x, y } => out.push((x, y)),
                SlotBlock::Grid {
                    left,
                    top,
                    cols,
                    rows,
                } => {
                    for y in 0..rows as i16 {
                        for x in 0..cols as i16 {
                            out.push((left + x * SLOT_PITCH, top + y * SLOT_PITCH));
                        }
                    }
                }
                SlotBlock::StandardInventory { left, top } => {
                    for y in 0..3i16 {
                        for x in 0..9i16 {
                            out.push((left + x * SLOT_PITCH, top + y * SLOT_PITCH));
                        }
                    }
                    for x in 0..9i16 {
                        out.push((left + x * SLOT_PITCH, top + TOP_TO_HOTBAR));
                    }
                }
            }
        }
        out
    }
}

/// A chest-family layout. `ChestMenu` is `addChestGrid(container, 8, 18)` then
/// `addStandardInventorySlots(inventory, 8, 18 + rows * 18 + 13)`, and
/// `ContainerScreen` is `176 x (114 + rows * 18)`.
const fn chest(protocol_id: i32, name: &'static str, rows: u8) -> MenuLayout {
    MenuLayout {
        protocol_id,
        name,
        blocks: match rows {
            1 => &CHEST_1,
            2 => &CHEST_2,
            3 => &CHEST_3,
            4 => &CHEST_4,
            5 => &CHEST_5,
            _ => &CHEST_6,
        },
        image_w: 176,
        image_h: 114 + (rows as i32) * 18,
    }
}

macro_rules! chest_blocks {
    ($name:ident, $rows:expr) => {
        const $name: [SlotBlock; 2] = [
            SlotBlock::Grid {
                left: 8,
                top: 18,
                cols: 9,
                rows: $rows,
            },
            SlotBlock::StandardInventory {
                left: 8,
                top: 18 + $rows * 18 + 13,
            },
        ];
    };
}
chest_blocks!(CHEST_1, 1);
chest_blocks!(CHEST_2, 2);
chest_blocks!(CHEST_3, 3);
chest_blocks!(CHEST_4, 4);
chest_blocks!(CHEST_5, 5);
chest_blocks!(CHEST_6, 6);

// --- the non-chest menus, one const per menu, in registry-id order ----------

/// `DispenserMenu`: `add3x3GridSlots(dispenser, 62, 17)`, then the player's.
const GENERIC_3X3: [SlotBlock; 2] = [
    SlotBlock::Grid {
        left: 62,
        top: 17,
        cols: 3,
        rows: 3,
    },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// `CrafterMenu.addSlots`: grid, **then the player**, then the result at 45.
/// See the module docs — this is the one menu whose own slot follows the
/// player's.
const CRAFTER_3X3: [SlotBlock; 3] = [
    SlotBlock::Grid {
        left: 26,
        top: 17,
        cols: 3,
        rows: 3,
    },
    SlotBlock::StandardInventory { left: 8, top: 84 },
    SlotBlock::One { x: 134, y: 35 },
];

/// `AnvilMenu.createInputSlotDefinitions` via `ItemCombinerMenu`.
const ANVIL: [SlotBlock; 4] = [
    SlotBlock::One { x: 27, y: 47 },
    SlotBlock::One { x: 76, y: 47 },
    SlotBlock::One { x: 134, y: 47 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// `BeaconMenu`: the payment slot, and the player's inventory at an unusual
/// origin — `(36, 137)`, not `(8, 84)`.
const BEACON: [SlotBlock; 2] = [
    SlotBlock::One { x: 136, y: 110 },
    SlotBlock::StandardInventory {
        left: 36,
        top: 137,
    },
];

/// `AbstractFurnaceMenu`: ingredient, fuel, result. Shared by all three
/// furnaces — they add no slots of their own.
const FURNACE: [SlotBlock; 4] = [
    SlotBlock::One { x: 56, y: 17 },
    SlotBlock::One { x: 56, y: 53 },
    SlotBlock::One { x: 116, y: 35 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// `BrewingStandMenu`: three potion slots, the ingredient, then the fuel.
const BREWING_STAND: [SlotBlock; 6] = [
    SlotBlock::One { x: 56, y: 51 },
    SlotBlock::One { x: 79, y: 58 },
    SlotBlock::One { x: 102, y: 51 },
    SlotBlock::One { x: 79, y: 17 },
    SlotBlock::One { x: 17, y: 17 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// `CraftingMenu`: the **result first** (slot 0), then the 3x3 grid, then the
/// player's. The opposite order to `crafter_3x3`.
const CRAFTING: [SlotBlock; 3] = [
    SlotBlock::One { x: 124, y: 35 },
    SlotBlock::Grid {
        left: 30,
        top: 17,
        cols: 3,
        rows: 3,
    },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

const ENCHANTMENT: [SlotBlock; 3] = [
    SlotBlock::One { x: 15, y: 47 },
    SlotBlock::One { x: 35, y: 47 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

const GRINDSTONE: [SlotBlock; 4] = [
    SlotBlock::One { x: 49, y: 19 },
    SlotBlock::One { x: 49, y: 40 },
    SlotBlock::One { x: 129, y: 34 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// `HopperMenu`: five slots in a row at `44 + x * 18, 20`, and the player's at
/// `(8, 51)` — the shallowest menu in the registry.
const HOPPER: [SlotBlock; 2] = [
    SlotBlock::Grid {
        left: 44,
        top: 20,
        cols: 5,
        rows: 1,
    },
    SlotBlock::StandardInventory { left: 8, top: 51 },
];

/// `LecternMenu`: one slot, **no player inventory**. See the module docs.
const LECTERN: [SlotBlock; 1] = [SlotBlock::One { x: 0, y: 0 }];

const LOOM: [SlotBlock; 5] = [
    SlotBlock::One { x: 13, y: 26 },
    SlotBlock::One { x: 33, y: 26 },
    SlotBlock::One { x: 23, y: 45 },
    SlotBlock::One { x: 143, y: 57 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// `MerchantMenu`: the player's inventory sits at `left = 108`, and the result
/// slot at `x = 220` is off the right of a 176-wide screen — `MerchantScreen`
/// is 276 wide.
const MERCHANT: [SlotBlock; 4] = [
    SlotBlock::One { x: 136, y: 37 },
    SlotBlock::One { x: 162, y: 37 },
    SlotBlock::One { x: 220, y: 37 },
    SlotBlock::StandardInventory {
        left: 108,
        top: 84,
    },
];

/// `ShulkerBoxMenu`: a 9x3 grid like a chest, but its own screen and its own
/// `ShulkerBoxSlot` (which refuses nested shulker boxes).
const SHULKER_BOX: [SlotBlock; 2] = [
    SlotBlock::Grid {
        left: 8,
        top: 18,
        cols: 9,
        rows: 3,
    },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

const SMITHING: [SlotBlock; 5] = [
    SlotBlock::One { x: 8, y: 48 },
    SlotBlock::One { x: 26, y: 48 },
    SlotBlock::One { x: 44, y: 48 },
    SlotBlock::One { x: 98, y: 48 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

const CARTOGRAPHY_TABLE: [SlotBlock; 4] = [
    SlotBlock::One { x: 15, y: 15 },
    SlotBlock::One { x: 15, y: 52 },
    SlotBlock::One { x: 145, y: 39 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

const STONECUTTER: [SlotBlock; 3] = [
    SlotBlock::One { x: 20, y: 33 },
    SlotBlock::One { x: 143, y: 33 },
    SlotBlock::StandardInventory { left: 8, top: 84 },
];

/// The player's own `InventoryMenu` — **container id 0**, and deliberately not
/// in [`REGISTRY`].
///
/// It has no `minecraft:menu` protocol id because `open_screen` never opens it:
/// it exists for the whole session, is created client-side with the player, and
/// is the menu `container_set_slot` writes whenever the id is 0, whatever else
/// is open. So it carries [`NO_PROTOCOL_ID`] and is reached by name.
///
/// Transcribed from `InventoryMenu`'s constructor, which is the only place
/// these numbers exist — there is no data file:
///
/// ```text
/// addResultSlot(owner, 154, 28)                -> slot 0
/// addCraftingGridSlots(98, 18)                 -> slots 1..=4   (a 2x2)
/// ArmorSlot(..., 39 - i, 8, 8 + i * 18)        -> slots 5..=8
/// addStandardInventorySlots(inventory, 8, 84)  -> slots 9..=44
/// Slot(inventory, 40, 77, 62)                  -> slot 45        (offhand)
/// ```
///
/// The armour column is a `Grid` of one column and four rows rather than four
/// `One`s, because that is what it is; note its **backing** indices run
/// backwards (`39 - i`, head-first), which is M69's finding and is a property
/// of the container behind the slots, not of where they are drawn.
pub static PLAYER: MenuLayout = MenuLayout {
    protocol_id: NO_PROTOCOL_ID,
    name: "player",
    blocks: &PLAYER_BLOCKS,
    image_w: 176,
    image_h: 166,
};

const PLAYER_BLOCKS: [SlotBlock; 5] = [
    SlotBlock::One { x: 154, y: 28 },
    SlotBlock::Grid {
        left: 98,
        top: 18,
        cols: 2,
        rows: 2,
    },
    SlotBlock::Grid {
        left: 8,
        top: 8,
        cols: 1,
        rows: 4,
    },
    SlotBlock::StandardInventory { left: 8, top: 84 },
    SlotBlock::One { x: 77, y: 62 },
];

/// Every `minecraft:menu` type, indexed by registry id.
///
/// The registry is dense and 0-based, so `REGISTRY[id]` is the lookup and
/// `protocol_id` is a redundant field kept so a reordering fails a test rather
/// than silently renumbering every menu.
pub static REGISTRY: &[MenuLayout] = &[
    chest(0, "generic_9x1", 1),
    chest(1, "generic_9x2", 2),
    chest(2, "generic_9x3", 3),
    chest(3, "generic_9x4", 4),
    chest(4, "generic_9x5", 5),
    chest(5, "generic_9x6", 6),
    MenuLayout {
        protocol_id: 6,
        name: "generic_3x3",
        blocks: &GENERIC_3X3,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 7,
        name: "crafter_3x3",
        blocks: &CRAFTER_3X3,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 8,
        name: "anvil",
        blocks: &ANVIL,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 9,
        name: "beacon",
        blocks: &BEACON,
        image_w: 230,
        image_h: 219,
    },
    MenuLayout {
        protocol_id: 10,
        name: "blast_furnace",
        blocks: &FURNACE,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 11,
        name: "brewing_stand",
        blocks: &BREWING_STAND,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 12,
        name: "crafting",
        blocks: &CRAFTING,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 13,
        name: "enchantment",
        blocks: &ENCHANTMENT,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 14,
        name: "furnace",
        blocks: &FURNACE,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 15,
        name: "grindstone",
        blocks: &GRINDSTONE,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 16,
        name: "hopper",
        blocks: &HOPPER,
        image_w: 176,
        image_h: 133,
    },
    MenuLayout {
        protocol_id: 17,
        name: "lectern",
        blocks: &LECTERN,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 18,
        name: "loom",
        blocks: &LOOM,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 19,
        name: "merchant",
        blocks: &MERCHANT,
        image_w: 276,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 20,
        name: "shulker_box",
        blocks: &SHULKER_BOX,
        image_w: 176,
        image_h: 167,
    },
    MenuLayout {
        protocol_id: 21,
        name: "smithing",
        blocks: &SMITHING,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 22,
        name: "smoker",
        blocks: &FURNACE,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 23,
        name: "cartography_table",
        blocks: &CARTOGRAPHY_TABLE,
        image_w: 176,
        image_h: 166,
    },
    MenuLayout {
        protocol_id: 24,
        name: "stonecutter",
        blocks: &STONECUTTER,
        image_w: 176,
        image_h: 166,
    },
];

/// Resolve a `minecraft:menu` registry id.
///
/// Returns `None` for an unknown id rather than substituting a default: a menu
/// Rewo cannot lay out must not be drawn as some other menu, because the slot
/// indices would be wrong and the player would be clicking the wrong things.
pub fn layout_of(protocol_id: i32) -> Option<&'static MenuLayout> {
    usize::try_from(protocol_id)
        .ok()
        .and_then(|i| REGISTRY.get(i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_dense_and_in_id_order() {
        assert_eq!(REGISTRY.len(), 25, "minecraft:menu has 25 entries in 26.2");
        for (i, m) in REGISTRY.iter().enumerate() {
            assert_eq!(
                m.protocol_id, i as i32,
                "{} is at index {i} but declares id {}",
                m.name, m.protocol_id
            );
        }
    }

    #[test]
    fn positions_agree_with_slot_count() {
        for m in REGISTRY {
            assert_eq!(
                m.positions().len(),
                m.slot_count(),
                "{}: expansion and count disagree",
                m.name
            );
        }
    }

    #[test]
    fn the_chest_family_is_nine_by_rows_plus_the_player() {
        // generic_9x1 .. 9x6 are 9*rows container slots + the player's 36.
        for (rows, m) in REGISTRY[0..6].iter().enumerate() {
            let rows = rows + 1;
            assert_eq!(m.slot_count(), 9 * rows + 36, "{}", m.name);
            assert_eq!(m.image_h, 114 + rows as i32 * 18, "{}", m.name);
        }
    }

    #[test]
    fn the_crafter_result_slot_follows_the_player_inventory() {
        // The inversion the module docs call out: grid 0..8, player 9..44,
        // result 45. If this ever reads 9, someone "tidied" the block order
        // and the crafter's output now lands in the player's inventory.
        let crafter = layout_of(7).unwrap();
        assert_eq!(crafter.slot_count(), 9 + 36 + 1);
        let pos = crafter.positions();
        assert_eq!(pos[45], (134, 35), "the result slot is the LAST slot");
        assert_eq!(pos[0], (26, 17), "slot 0 is the top-left grid cell");
    }

    #[test]
    fn the_lectern_has_one_slot_and_no_player_inventory() {
        let lectern = layout_of(17).unwrap();
        assert_eq!(lectern.slot_count(), 1);
        assert!(
            !lectern
                .blocks
                .iter()
                .any(|b| matches!(b, SlotBlock::StandardInventory { .. })),
            "LecternMenu never calls addStandardInventorySlots"
        );
    }

    #[test]
    fn the_hotbar_sits_58_below_its_origin_not_54() {
        // topToHotbar is a named 58, leaving a 4px gap after the 3x18 block.
        // At 54 the hotbar would overlap the row above it.
        let chest = layout_of(2).unwrap(); // generic_9x3
        let pos = chest.positions();
        let extended_last_row_y = pos[27 + 18].1; // first slot of the 3rd extended row
        let hotbar_y = pos[27 + 27].1; // first hotbar slot
        assert_eq!(hotbar_y - extended_last_row_y, 58 - 2 * 18);
        assert_eq!(hotbar_y - pos[27].1, TOP_TO_HOTBAR);
    }

    #[test]
    fn an_unknown_menu_id_resolves_to_none_never_a_default() {
        assert!(layout_of(25).is_none());
        assert!(layout_of(-1).is_none());
        assert!(layout_of(i32::MAX).is_none());
    }
}
