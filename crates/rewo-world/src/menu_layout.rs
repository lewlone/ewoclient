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

/// How a menu routes a shift-click.
///
/// `AbstractContainerMenu.quickMoveStack` is a **per-menu-class override**, not
/// a parameter — `ChestMenu`'s differs from `InventoryMenu`'s, and the furnace
/// and crafting families differ again — so this is a transcription per shape,
/// not a formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickMove {
    /// The player's own `InventoryMenu`: armour first, then the main/hotbar
    /// cross-move, with the crafting result filling backwards.
    PlayerInventory,
    /// The shape `ChestMenu`, `ShulkerBoxMenu`, `DispenserMenu` and
    /// `HopperMenu` share, differing only in how many slots the container has:
    ///
    /// ```java
    /// if (slotIndex < containerSize) moveItemStackTo(stack, containerSize, slots.size(), true);
    /// else                           moveItemStackTo(stack, 0, containerSize, false);
    /// ```
    ///
    /// Container to player fills **backwards** — from the top of the range,
    /// which in these menus is the hotbar's right-hand end, because
    /// `addStandardInventorySlots` appends the hotbar last.
    SimpleContainer { container_slots: usize },
    /// `AbstractFurnaceMenu`: ingredient 0, fuel 1, result 2, then the
    /// player's 36. Needs the item to decide, unlike every other shape —
    /// `canSmelt` is checked **before** `isFuel`, and a log is both, so a log
    /// goes to the INGREDIENT slot. Routing on `isFuel` alone puts it in the
    /// fuel slot, which is wrong and looks reasonable.
    Furnace,
    /// `CraftingMenu` — result 0, grid 1..=9, then the player's 36 (M92e).
    ///
    /// The only shape whose player-slot branch is a **fallback chain**: it
    /// tries the crafting grid first and cross-moves between the main
    /// inventory and the hotbar only if the grid took nothing. That is why
    /// shift-clicking in a crafting table *fills the grid* — behaviour
    /// `InventoryMenu` does not have, because its own 2x2 grid is not a
    /// shift-click destination at all.
    Crafting,
    /// `CrafterMenu` — grid 0..=8, the player's 36, then the result at 45.
    ///
    /// Almost [`QuickMove::SimpleContainer`], and the one difference is the
    /// number that matters: its player destination is `9..45`, **not**
    /// `9..slots.size()`. Slot 45 is the `NonInteractiveResultSlot`, and
    /// vanilla excludes it from the range outright rather than relying on
    /// `mayPlace` to refuse it.
    Crafter { container_slots: usize },
    /// `MerchantMenu` — trade inputs 0 and 1, result 2, then the player's 36.
    ///
    /// The **only** menu in the registry whose `quickMoveStack` consults
    /// nothing at all: no item predicate, no recipe, no container state. That
    /// matters because it contradicts the obvious reading of "the merchant is
    /// blocked on `merchant_offers`" — the trade *list* is, and the quick-move
    /// is not, because it never routes a player stack into slots 0 or 1. A
    /// shift-click from the player inventory does the main/hotbar cross-move
    /// and nothing else; vanilla will not load your emeralds into a trade for
    /// you.
    Merchant,
    /// `ItemCombinerMenu` — `n` input slots, then the result, then the
    /// player's 36. Anvil (`result_slot` 2) and smithing (3).
    ///
    /// # The branch that is a guard, not a fallback
    ///
    /// Its player-slot arm reads
    ///
    /// ```java
    /// } else if (canMoveIntoInputSlots(stack) && slotIndex >= invStart && slotIndex < useRowEnd) {
    ///     if (!moveItemStackTo(stack, 0, getResultSlot(), false)) return ItemStack.EMPTY;
    /// } else if (slotIndex >= invStart && slotIndex < invEnd) {  // main -> hotbar
    /// ```
    ///
    /// so the input-slot attempt **consumes** the branch rather than falling
    /// through to the cross-move the way [`QuickMove::Crafting`] does (M92e).
    /// `canMoveIntoInputSlots` defaults to `true`, so for the anvil the two
    /// main/hotbar arms are **structurally unreachable**: shift-clicking
    /// anywhere in your inventory with an anvil open either fills an input
    /// slot or moves nothing at all. That is the behaviour, not an omission.
    ///
    /// Only the **anvil** maps here. It inherits the `true` guard and both its
    /// input slots accept anything (`withSlot(0, .., itemStack -> true)`), so
    /// the transcription is exact with nothing looked up.
    ///
    /// **`SmithingMenu` is [`QuickMove::Smithing`] rather than this variant
    /// with a different `result_slot`** (M152), and the reason is not that its
    /// guard needs data — it is that a guard which can FAIL changes which arms
    /// run. See that variant.
    ItemCombiner { result_slot: usize },
    /// `SmithingMenu` — template 0, base 1, addition 2, result 3, then the
    /// player's 36 (M152).
    ///
    /// # Why not `ItemCombiner { result_slot: 3 }`
    ///
    /// Because **smithing is the only `ItemCombiner` in the game whose
    /// cross-move arms execute at all.** `ItemCombinerMenu.quickMoveStack` has
    /// five arms; arms 4 and 5 are the ordinary main<->hotbar cross-move and
    /// they sit *below* arm 3, whose condition is
    ///
    /// ```java
    /// } else if (canMoveIntoInputSlots(stack) && slotIndex >= invStart && slotIndex < useRowEnd) {
    /// ```
    ///
    /// Arms 1 and 2 already consume every index below `invStart`, so that range
    /// test is redundant and **the guard is arm 3's only live condition**. For
    /// the anvil the guard is the inherited `true`, so arm 3 always wins and
    /// arms 4/5 are dead — which is exactly what [`QuickMove::ItemCombiner`]
    /// encodes, and correctly. `SmithingMenu` overrides the guard
    /// (`SmithingMenu.java:134-140`), so it can fail, and then the two
    /// cross-move arms become reachable.
    ///
    /// Adding a guard to the combiner arm and returning its single branch
    /// would therefore be wrong in a way that only shows on this one menu: a
    /// shift-click the smithing table refuses would move **nothing**, where
    /// vanilla moves it between your main inventory and hotbar.
    ///
    /// # The guard is not a pure item predicate
    ///
    /// Each disjunct is conjoined with its target slot being **empty**:
    ///
    /// ```java
    /// if (templateItemTest.test(stack) && !getSlot(0).hasItem()) return true;
    /// else return baseItemTest.test(stack) && !getSlot(1).hasItem() ? true
    ///          : additionItemTest.test(stack) && !getSlot(2).hasItem();
    /// ```
    ///
    /// so it reads the menu's occupancy, not just the item — a second netherite
    /// ingot is refused while the first is still in the addition slot, and
    /// cross-moves instead.
    ///
    /// # The sets are wire-derived
    ///
    /// Unlike the beacon's tag or the stonecutter's jar table, the three
    /// `RecipePropertySet`s arrive on `update_recipes` (M152). Before it lands
    /// all three refuse everything, which is also what vanilla does —
    /// `getOrDefault(id, RecipePropertySet.EMPTY)`.
    Smithing,
    /// `BeaconMenu` — one payment slot, then the player's 36.
    ///
    /// Its guard is three conditions and only one of them is about the item:
    /// the payment slot must be **empty**, the stack must be in
    /// `ItemTags.BEACON_PAYMENT_ITEMS`, and `stack.getCount() == 1`. The count
    /// test is in the `quickMoveStack` branch itself rather than in
    /// `mayPlace`, so a stack of two diamonds cross-moves like an ordinary
    /// item while a single diamond is claimed by the beacon.
    ///
    /// Unlike the combiner, the guard failing here **does** fall through to
    /// the main/hotbar cross-move, because it is a separate `else if` rather
    /// than a condition on the destination.
    Beacon,
    /// `StonecutterMenu` — input 0, result 1, then the player's 36.
    ///
    /// Its guard is `stonecutterRecipes().acceptsInput(stack)`, and it is the
    /// **third** guard behaviour in three menus, which is why they could not
    /// share a shape:
    ///
    /// | menu | guard | when it fails |
    /// |---|---|---|
    /// | anvil | always `true` | n/a — the cross-move is unreachable |
    /// | beacon | tag + count + empty | falls through to the cross-move |
    /// | stonecutter | `acceptsInput` | falls through to the cross-move… |
    ///
    /// …but the *move* consuming is what differs: a stonecuttable stack whose
    /// input slot is already occupied by something else moves **nothing**,
    /// because `moveItemStackTo(stack, 0, 1, false)` returns false and vanilla
    /// then `return`s rather than trying the hotbar. The guard falling through
    /// and the move failing are two different exits from the same branch, and
    /// only the first reaches the cross-move.
    ///
    /// The accepted set is jar-derived (`stonecutter_table`), with M91's
    /// caveat: `update_recipes` is the authoritative source and is class C.
    Stonecutter,
    /// `LoomMenu` — banner 0, dye 1, pattern 2, result 3, player 4..40.
    ///
    /// Three input predicates tested in a fixed order, and each is also its
    /// slot's `mayPlace` — the cartography table's arrangement with one more
    /// slot. Two of the three are **conjunctions**: `isDyeItem` is
    /// `is(#LOOM_DYES) && has(DYE)`, not either. Every vanilla item satisfies
    /// both terms together, so a tag-only test looks correct and diverges the
    /// moment a datapack tags an item without the component.
    ///
    /// The banner test is `getItem() instanceof BannerItem` — a class, whose
    /// data stand-in must be `#minecraft:banners` and **not** the
    /// `minecraft:banner_patterns` component, which the shield carries too.
    Loom,
    /// `CartographyTableMenu` — map 0, additional 1, result 2, player 3..39.
    ///
    /// The only menu here whose branch predicates and `mayPlace` predicates are
    /// the **same two tests**, written twice. The branch picks which slot to
    /// try; `mayPlace` confirms it will take the stack. That redundancy is
    /// vanilla's, and it means neither can be dropped: without the branch a
    /// map would cross-move, without `mayPlace` an ordinary click could put a
    /// filled map in the paper slot.
    ///
    /// `has(MAP_ID)` is tested first, which is what separates `filled_map`
    /// (component present -> slot 0) from `minecraft:map` (a different item,
    /// no component -> slot 1). Putting both in one slot would break cloning.
    Cartography,
    /// `GrindstoneMenu` — repair 0 and 1, result 2, then the player's 36.
    ///
    /// **Its `quickMoveStack` has no item predicate at all.** The guard is
    /// `!input.isEmpty() && !additional.isEmpty()` — purely whether both repair
    /// slots are occupied — which is the opposite arrangement from every other
    /// menu here: the beacon and the stonecutter ask about the *item* in the
    /// branch and let the slot accept anything, while the grindstone asks about
    /// the *slots* in the branch and puts the item predicate in `mayPlace`.
    ///
    /// That is why [`crate::inventory::SlotKind::GrindstoneInput`] is
    /// load-bearing for the shift-click and not only for an ordinary click: it
    /// is the sole thing that stops a stick reaching a repair slot, via
    /// `moveItemStackTo`'s placement pass. And when it does stop it, the move
    /// returns false and vanilla `return`s — so shift-clicking a stick into an
    /// empty grindstone moves **nothing**, rather than cross-moving.
    Grindstone,
    /// Not transcribed yet. The caller must **decline** rather than fall back
    /// to another menu's routing: a shift-click sent under the wrong menu's
    /// rules moves the wrong stack to the wrong place and the server applies
    /// it, where declining moves nothing and is merely inert.
    Unimplemented,
}

impl MenuLayout {
    /// What kind of slot a menu index is, which is what decides its rules.
    ///
    /// Player-specific in vanilla only because `InventoryMenu` is the menu with
    /// result, crafting and armour slots — a chest's are all plain `Slot`s. An
    /// untranscribed menu answers `None` and every click on it declines, which
    /// is the same choice [`QuickMove::Unimplemented`] makes and for the same
    /// reason: an anvil's result slot refuses placement, and treating it as
    /// plain would let a click put something there that the server then
    /// rejects.
    pub fn slot_kind(&self, slot: usize) -> Option<crate::inventory::SlotKind> {
        if slot >= self.slot_count() {
            return None;
        }
        match self.quick_move() {
            QuickMove::PlayerInventory => crate::inventory::slot_kind(slot),
            QuickMove::SimpleContainer { .. } => Some(crate::inventory::SlotKind::Plain),
            // A furnace's slot 2 is a `FurnaceResultSlot` — it refuses
            // placement, so it must not read as plain.
            QuickMove::Furnace => Some(if slot == 2 {
                crate::inventory::SlotKind::Result
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // A crafting table's slot 0 is a `ResultSlot` — it refuses
            // placement, so it must not read as plain.
            QuickMove::Crafting => Some(if slot == 0 {
                crate::inventory::SlotKind::Result
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // ...and a crafter's result is its LAST slot, not its first. The
            // two crafting menus put it at opposite ends (see the module
            // docs), which is exactly the kind of thing a shared `Result`
            // constant would paper over.
            QuickMove::Crafter { .. } => Some(if slot + 1 == self.slot_count() {
                crate::inventory::SlotKind::Result
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // A merchant's slot 2 is a `MerchantResultSlot`, which extends
            // `Slot` and overrides `mayPlace` to `false` — same shape as a
            // furnace's, so the same `Result`.
            QuickMove::Merchant => Some(if slot == 2 {
                crate::inventory::SlotKind::Result
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // `createResultSlot` builds a slot whose `mayPlace` is `false`;
            // every input slot before it carries the definition's own
            // predicate, which for the anvil (the only menu mapped here) is
            // `itemStack -> true`, i.e. exactly `Plain`.
            // M152. Three distinct input kinds, because each slot enforces its
            // own `RecipePropertySet::test` (`SmithingMenu.java:57-61`) and the
            // three sets are disjoint over vanilla data. One shared kind would
            // let a netherite ingot into the template slot on a plain click.
            //
            // Slot 3 is `withResultSlot`, which refuses placement. The player's
            // 36 are `Plain`, matching every other container arm here.
            QuickMove::Smithing => Some(match slot {
                0 => crate::inventory::SlotKind::SmithingTemplate,
                1 => crate::inventory::SlotKind::SmithingBase,
                2 => crate::inventory::SlotKind::SmithingAddition,
                3 => crate::inventory::SlotKind::Result,
                _ => crate::inventory::SlotKind::Plain,
            }),
            QuickMove::ItemCombiner { result_slot } => Some(if slot == result_slot {
                crate::inventory::SlotKind::Result
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // The beacon's payment slot takes a TAG, so it is not plain: it is
            // the one container slot in the transcribed set that refuses an
            // ordinary item, and `SlotKind::BeaconPayment` is what lets a
            // plain click respect that too rather than only the quick-move.
            QuickMove::Beacon => Some(if slot == 0 {
                crate::inventory::SlotKind::BeaconPayment
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // A stonecutter's slot 1 overrides `mayPlace` to false; slot 0 is a
            // BARE `Slot` with no override, so it accepts anything an ordinary
            // click puts there. `acceptsInput` gates the shift-click ONLY —
            // giving slot 0 a kind of its own would be stricter than vanilla.
            QuickMove::Stonecutter => Some(if slot == 1 {
                crate::inventory::SlotKind::Result
            } else {
                crate::inventory::SlotKind::Plain
            }),
            // A grindstone's slot 2 is its result; 0 and 1 share the
            // damageable-or-enchanted predicate, so they are their own kind.
            QuickMove::Grindstone => Some(match slot {
                0 | 1 => crate::inventory::SlotKind::GrindstoneInput,
                2 => crate::inventory::SlotKind::Result,
                _ => crate::inventory::SlotKind::Plain,
            }),
            // M93f — two DIFFERENT kinds, because the two slots take disjoint
            // sets. Sharing one would let a filled map into the paper slot.
            // M93g — three kinds, because the three predicates are disjoint
            // and each slot enforces its own.
            QuickMove::Loom => Some(match slot {
                0 => crate::inventory::SlotKind::LoomBanner,
                1 => crate::inventory::SlotKind::LoomDye,
                2 => crate::inventory::SlotKind::LoomPattern,
                3 => crate::inventory::SlotKind::Result,
                _ => crate::inventory::SlotKind::Plain,
            }),
            QuickMove::Cartography => Some(match slot {
                0 => crate::inventory::SlotKind::CartographyMap,
                1 => crate::inventory::SlotKind::CartographyAdditional,
                2 => crate::inventory::SlotKind::Result,
                _ => crate::inventory::SlotKind::Plain,
            }),
            QuickMove::Unimplemented => None,
        }
    }

    /// This menu's shift-click routing.
    pub fn quick_move(&self) -> QuickMove {
        if self.protocol_id == NO_PROTOCOL_ID {
            return QuickMove::PlayerInventory;
        }
        match self.protocol_id {
            // generic_9x1..9x6, generic_3x3 (dispenser), hopper, shulker_box —
            // every one of them the player's 36 appended after the container's
            // own slots, which is what makes `slot_count() - 36` the split.
            0..=6 | 16 | 20 => QuickMove::SimpleContainer {
                container_slots: self.slot_count() - 36,
            },
            // blast_furnace, furnace, smoker — one shape, three accepted-input
            // sets, which `smelting_table::accepts` selects by this same id.
            10 | 14 | 22 => QuickMove::Furnace,
            // crafting (M92e) — the fallback-chain shape.
            12 => QuickMove::Crafting,
            // crafter_3x3 (M92e) — a 9-slot container whose result sits AFTER
            // the player's inventory, so the split is not `slot_count() - 36`.
            7 => QuickMove::Crafter { container_slots: 9 },
            // M93 — the single-input family.
            //
            // anvil: the ItemCombinerMenu shape, `withResultSlot(2, ...)`.
            8 => QuickMove::ItemCombiner { result_slot: 2 },
            // smithing (M152) — the same class at result 3, but NOT the same
            // arm: its overridden guard can fail, which makes the cross-move
            // arms live. See `QuickMove::Smithing`.
            21 => QuickMove::Smithing,
            9 => QuickMove::Beacon,
            19 => QuickMove::Merchant,
            // stonecutter (M93b) — the jar-derived accepted-input set.
            24 => QuickMove::Stonecutter,
            // grindstone (M93e) — the predicate lives in `mayPlace`.
            15 => QuickMove::Grindstone,
            // cartography_table (M93f).
            23 => QuickMove::Cartography,
            // loom (M93g).
            18 => QuickMove::Loom,
            _ => QuickMove::Unimplemented,
        }
    }
}

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
