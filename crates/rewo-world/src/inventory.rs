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
//! `set_held_slot` speaks the *second* one.
//! [`menu_slot_of_inventory_index`] is the only place that conversion happens,
//! and [`Inventory::hotbar`] is one caller of it.
//!
//! ## The armour ranges run in opposite directions (M69)
//!
//! Both coordinate systems have four contiguous armour slots, and **they are
//! ordered against each other**. `InventoryMenu`'s constructor is
//!
//! ```text
//! SLOT_IDS = { HEAD, CHEST, LEGS, FEET };
//! for (i = 0; i < 4; i++) addSlot(new ArmorSlot(inventory, owner, SLOT_IDS[i], 39 - i, …));
//! ```
//!
//! so menu slot `5 + i` is backed by inventory index `39 - i`: helmet is menu
//! `5` / index `39`, boots are menu `8` / index `36`. Arithmetic that reads
//! "armour starts at 36 in one and at 5 in the other" and subtracts 31 puts
//! **boots on the head**, and the resulting render is a plausible-looking
//! wrong answer rather than an error. See [`menu_slot_of_inventory_index`].
//!
//! Two inventory indices have no menu slot at all: `41` `SLOT_BODY_ARMOR` and
//! `42` `SLOT_SADDLE` live only in `EntityEquipment`, and `InventoryMenu` has
//! 46 slots with no room for either. They are a third state, not an error —
//! [`IndexWrite::NoMenuSlot`].
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
    /// Whether the stack's `DataComponentPatch` carried any entry.
    pub has_components: bool,
    /// A digest of the stack's whole patch (M41).
    ///
    /// `0` for a stack with no patch. Two stacks are the same components iff
    /// this matches, which is what `ItemStack.isSameItemSameComponents` asks
    /// and what M35 could only approximate: before the component codecs were
    /// transcribed, "carries any component at all" was the only honest answer,
    /// so every patched stack swapped rather than merging.
    pub components: u64,
    /// `minecraft:damage`, for the durability bar (M41). `None` is an
    /// undamaged stack — the component is absent until something wears it.
    pub damage: Option<i32>,
    /// `minecraft:max_damage` **when the patch overrides it**. Usually `None`,
    /// because the item's prototype carries the real maximum; a bar therefore
    /// needs the item table too.
    pub max_damage: Option<i32>,
    /// Whether the patch carried `minecraft:enchantments` with at least one
    /// entry — `ItemStack.isEnchanted`, which is what the glint and the
    /// tooltip's enchantment lines key on.
    pub enchanted: bool,
    /// `minecraft:trim`'s material registry id (M49), for picking the icon
    /// variant. The pattern is not here: an item definition's `select` is on
    /// `minecraft:trim_material` alone, so the pattern changes the worn model
    /// and never the icon.
    pub trim_material: Option<i32>,
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

// ---------------------------------------------------------------------------
// The inventory-index coordinate system (M69).
//
// Everything below is the *other* address space — the one
// `set_player_inventory` and `Inventory.setItem` speak. It is kept in this one
// place so no caller is tempted to do the arithmetic itself.
// ---------------------------------------------------------------------------

/// `Inventory.INVENTORY_SIZE` — the backing `NonNullList` is 36 long, and
/// `Inventory.setItem`'s first branch is `if (slot < this.items.size())`.
/// Indices at or past this are equipment, not storage.
pub const INVENTORY_ITEMS_SIZE: i32 = 36;
/// `EquipmentSlot.FEET.getIndex(36)` — the first of the four humanoid armour
/// indices, and the one holding **boots**. See the module header.
pub const ARMOR_INDEX_START: i32 = 36;
/// `Inventory.SLOT_OFFHAND`.
pub const OFFHAND_INDEX: i32 = 40;
/// `Inventory.SLOT_BODY_ARMOR` — an `EntityEquipment` slot with no
/// `InventoryMenu` counterpart.
pub const BODY_ARMOR_INDEX: i32 = 41;
/// `Inventory.SLOT_SADDLE` — likewise.
pub const SADDLE_INDEX: i32 = 42;

/// What an inventory index addresses in Rewo's 46-slot menu array.
///
/// Three outcomes rather than two, because "vanilla writes this somewhere Rewo
/// has no room for" and "vanilla writes this nowhere" are different facts and
/// collapsing them would make the first look like a decode failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexWrite {
    /// Lands in this menu slot.
    Applied(usize),
    /// A real `EntityEquipment` slot — `SLOT_BODY_ARMOR` (41) or
    /// `SLOT_SADDLE` (42). `Inventory.setItem` stores both; `InventoryMenu`
    /// exposes neither, so Rewo's 46-slot array has nowhere to put it.
    NoMenuSlot,
    /// Outside every index `Inventory.setItem` maps.
    ///
    /// Vanilla is not merely quiet here: `setItem(-1, …)` passes the
    /// `slot < items.size()` guard and then throws `IndexOutOfBoundsException`
    /// out of `NonNullList.set`, which drops the connection. Rewo reports the
    /// index instead — a deliberate deviation, because a hostile VarInt should
    /// not be able to end the session.
    OutOfRange,
}

/// One inventory index → the menu slot backing it, or why there isn't one.
///
/// The whole table, from `InventoryMenu`'s constructor and
/// `Inventory.EQUIPMENT_SLOT_MAPPING`:
///
/// | inventory index | menu slot | what |
/// |---|---|---|
/// | `0..9` | `36..45` | hotbar |
/// | `9..36` | `9..36` | main inventory (**identity**) |
/// | `36` | `8` | feet |
/// | `37` | `7` | legs |
/// | `38` | `6` | chest |
/// | `39` | `5` | head |
/// | `40` | `45` | off-hand |
/// | `41`, `42` | — | body armour, saddle |
///
/// Menu slots `0..5` — the crafting result and the 2×2 grid — are backed by
/// the menu's own `CraftingContainer`, not by `Inventory` at all, so **no**
/// inventory index maps to them. A server cannot reach your crafting grid with
/// this packet.
pub fn menu_slot_of_inventory_index(index: i32) -> IndexWrite {
    match index {
        // `addInventoryHotbarSlots` adds `Slot(inventory, x, …)` for x in 0..9
        // *after* the extended slots, so they occupy menu 36..45.
        0..=8 => IndexWrite::Applied(HOTBAR_MENU_START + index as usize),
        // `addInventoryExtendedSlots` adds `Slot(inventory, x + (y+1)*9, …)`
        // in the same order, so index and menu slot coincide over 9..36.
        9..=35 => IndexWrite::Applied(index as usize),
        // `39 - i` for menu slot `5 + i`: the two ranges run opposite ways.
        // Written as the arithmetic vanilla's loop performs, inverted, rather
        // than as four literals — a literal table is right until someone
        // "simplifies" it back into a subtraction.
        36..=39 => IndexWrite::Applied(ARMOR_MENU_START + (39 - index) as usize),
        OFFHAND_INDEX => IndexWrite::Applied(OFFHAND_MENU_SLOT),
        BODY_ARMOR_INDEX | SADDLE_INDEX => IndexWrite::NoMenuSlot,
        _ => IndexWrite::OutOfRange,
    }
}

#[derive(Clone, Debug)]
pub struct Inventory {
    /// Which menu this is. `PLAYER` for the player's own `InventoryMenu`
    /// (container id 0); one of [`crate::menu_layout::REGISTRY`] for a
    /// container opened by `open_screen`.
    ///
    /// This is the type vanilla calls `AbstractContainerMenu`: there, the
    /// player's inventory is not a distinct class, it is `InventoryMenu` —
    /// *a* menu, with a particular slot list. The name here stays `Inventory`
    /// because it was that before it was general and ~110 call sites say so;
    /// what changed is that it is no longer only the player's.
    layout: &'static crate::menu_layout::MenuLayout,
    /// One entry per menu slot, in wire order. Length is `layout.slot_count()`
    /// — 46 for the player, 90 for a double chest, **1 for a lectern**.
    slots: Vec<Option<ItemSlot>>,
    /// The stack on the cursor. Decoded because the packet carries it; nothing
    /// renders it yet (it is only visible with an inventory screen open).
    carried: Option<ItemSlot>,
    /// `Inventory.selectedSlot`, an **inventory index** in `0..9`.
    selected: u8,
    /// The server's `stateId` for the last content/slot update applied, echoed
    /// back by every serverbound click.
    state_id: i32,
    /// How many whole-container updates have arrived (M35).
    ///
    /// The server sends one when it *disagrees* with a click's prediction, so
    /// counting them is how a gate tells an accepted click from a corrected
    /// one — the container equivalent of the physics harness's `CORRECTIONS`.
    /// It also sends one on join and on any inventory change it originates,
    /// so the count is only meaningful as a delta across a click.
    content_updates: u32,
    /// Tooltip text by component fingerprint (M41) — see [`SlotText`].
    texts: std::collections::HashMap<u64, SlotText>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::with_layout(&crate::menu_layout::PLAYER)
    }
}

impl Inventory {
    /// A menu of the given layout, all slots empty.
    ///
    /// `Default` is this with [`crate::menu_layout::PLAYER`], which is the
    /// player's permanent container-id-0 menu.
    pub fn with_layout(layout: &'static crate::menu_layout::MenuLayout) -> Self {
        Self {
            layout,
            slots: vec![None; layout.slot_count()],
            carried: None,
            selected: 0,
            state_id: 0,
            content_updates: 0,
            texts: std::collections::HashMap::new(),
        }
    }

    /// This menu's layout.
    pub fn layout(&self) -> &'static crate::menu_layout::MenuLayout {
        self.layout
    }

    /// How many slots this menu has. `MENU_SLOTS` for the player.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
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
        if slots.len() != self.slots.len() {
            return false;
        }
        self.slots.copy_from_slice(slots);
        self.carried = carried;
        self.state_id = state_id;
        self.content_updates += 1;
        true
    }

    /// `handleContainerSetSlot` for container 0. An out-of-range slot is
    /// ignored, not clamped.
    pub fn set_slot(&mut self, state_id: i32, slot: i32, item: Option<ItemSlot>) -> bool {
        let Ok(idx) = usize::try_from(slot) else {
            return false;
        };
        if idx >= self.slots.len() {
            return false;
        }
        self.slots[idx] = item;
        self.state_id = state_id;
        true
    }

    /// `handleSetPlayerInventory` (M69) — an authoritative write addressed by
    /// **inventory index**, not menu slot.
    ///
    /// `ClientPacketListener.handleSetPlayerInventory` is
    /// `player.getInventory().setItem(packet.slot(), packet.contents())`, and
    /// that is the whole handler. Two consequences the sibling
    /// [`Inventory::set_slot`] does not share:
    ///
    /// * **There is no state id on this packet**, so this must not touch
    ///   [`Inventory::state_id`]. The state id is the container menu's
    ///   click-prediction sequence; `Inventory.setItem` bypasses the menu
    ///   entirely. Advancing it from here would make the next click echo a
    ///   number the server never issued, and the server answers a stale state
    ///   id with the full resync this packet exists to avoid.
    /// * **The index is not a menu slot**, so it goes through
    ///   [`menu_slot_of_inventory_index`] and nothing else.
    pub fn set_inventory_index(&mut self, index: i32, item: Option<ItemSlot>) -> IndexWrite {
        let write = menu_slot_of_inventory_index(index);
        if let IndexWrite::Applied(slot) = write {
            self.slots[slot] = item;
        }
        write
    }

    /// `handleSetCursorItem` (M69) — `containerMenu.setCarried(contents)`.
    ///
    /// The server's authoritative answer for the carried stack, and the only
    /// correction path M35's predicted cursor has short of a whole
    /// `container_set_content`. Carries **no state id and no container id**:
    /// vanilla's only guard is that the open screen is not the creative
    /// inventory, which Rewo has no notion of.
    pub fn set_carried(&mut self, item: Option<ItemSlot>) {
        self.carried = item;
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

    /// See [`Self::content_updates`] — a delta across a click of more than
    /// zero means the server rejected the prediction.
    pub fn content_updates(&self) -> u32 {
        self.content_updates
    }

    pub fn carried(&self) -> Option<ItemSlot> {
        self.carried
    }

    /// One hotbar slot by **inventory index** (`0..9`).
    ///
    /// Goes through [`menu_slot_of_inventory_index`] rather than adding
    /// `HOTBAR_MENU_START` itself: this was the original conversion site, and
    /// M69's `set_player_inventory` needed the same table over a wider range.
    /// Two copies of an index→slot map is exactly the drift M62 recorded, so
    /// there is one, and the narrow caller reads it.
    pub fn hotbar(&self, index: usize) -> Option<ItemSlot> {
        let Ok(index) = i32::try_from(index) else {
            return None;
        };
        if !Self::is_hotbar_index(index) {
            return None;
        }
        match menu_slot_of_inventory_index(index) {
            IndexWrite::Applied(slot) => self.slots[slot],
            // Unreachable: `is_hotbar_index` already bounded it to `0..9`.
            _ => None,
        }
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

/// The text a stack's components contribute to its tooltip (M41).
///
/// Kept beside [`Inventory`] rather than inside [`ItemSlot`] because the slot
/// is `Copy` and the click arithmetic moves it through a dozen expressions;
/// growing it to own three strings would make every one of those a clone.
///
/// Keyed by the **component fingerprint**, which is the reason this works at
/// all: the text is derived from the patch, so two slots holding the same
/// components share one entry, and a locally-predicted click that moves a
/// stack from one slot to another carries its text with it for free — the
/// fingerprint travels in the `ItemSlot`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotText {
    /// `custom_name` if present, else `item_name`. Either overrides the item's
    /// translated name; the caller supplies that fallback.
    pub name: Option<String>,
    pub lore: Vec<String>,
    pub rarity: Option<i32>,
    pub unbreakable: bool,
    /// `(enchantment protocol id, level)` from `minecraft:enchantments`, and
    /// from `minecraft:stored_enchantments` for a book (M42).
    ///
    /// Ids, not names: translating them needs the runtime enchantment
    /// registry, which the *renderer* holds — this is the wire's own answer,
    /// carried no further than it can be trusted.
    pub enchantments: Vec<(i32, i32)>,
    /// `ItemStack.isEnchanted()` — `minecraft:enchantments` alone, **not** the
    /// union above (M50).
    ///
    /// The two differ for exactly one thing a player holds: an enchanted book
    /// carries `stored_enchantments` and is *not* enchanted, so its rarity is
    /// not promoted.
    pub is_enchanted: bool,
    /// `minecraft:use_cooldown`'s `cooldownGroup` (M79) — the key
    /// `ItemCooldowns` indexes this stack by, when the stack overrides it.
    ///
    /// `None` means "use the item's registry name", which is
    /// `getCooldownGroup`'s `orElse` and covers both an absent component and a
    /// present one with an empty `Optional`.
    ///
    /// Not tooltip text, unlike everything above it — it rides here because
    /// this is the per-fingerprint carrier for *interpreted components* and
    /// `ItemSlot` is `Copy`, which is the same reason the tooltip text is not
    /// on the slot either.
    pub cooldown_group: Option<String>,
}

impl SlotText {
    /// Whether this contributes nothing to a tooltip.
    ///
    /// **Every field has to be listed.** `record_text` drops an empty one, so
    /// a field missing from here is a whole class of stack whose tooltip
    /// silently loses its lines — which is exactly what happened to the
    /// enchantments before they were added.
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.lore.is_empty()
            && self.rarity.is_none()
            && !self.unbreakable
            && self.enchantments.is_empty()
            && !self.is_enchanted
            && self.cooldown_group.is_none()
    }
}

/// How many distinct component sets keep their text.
///
/// One entry per *patch*, not per slot, so an ordinary inventory holds a
/// handful. The cap exists because a server that sends thousands of distinct
/// patches would otherwise grow this without bound; past it the oldest are
/// dropped and those stacks fall back to their translated name, which is a
/// missing tooltip line rather than a wrong one.
const MAX_SLOT_TEXTS: usize = 512;

impl Inventory {
    /// Remember what a component set says, so a tooltip can read it back.
    pub fn record_text(&mut self, fingerprint: u64, text: SlotText) {
        if fingerprint == 0 || text.is_empty() {
            return;
        }
        if self.texts.len() >= MAX_SLOT_TEXTS {
            self.texts.clear();
        }
        self.texts.insert(fingerprint, text);
    }

    /// What this stack's components say, if anything was recorded.
    pub fn text_of(&self, stack: ItemSlot) -> Option<&SlotText> {
        self.texts.get(&stack.components)
    }
}

// ---------------------------------------------------------------------------
// The screen (M35): where each slot sits, what it accepts, and what a click
// does to it.
// ---------------------------------------------------------------------------

/// `AbstractContainerScreen.imageWidth` for the player inventory.
pub const GUI_WIDTH: i32 = 176;
/// `AbstractContainerScreen.imageHeight`.
pub const GUI_HEIGHT: i32 = 166;

/// What kind of slot a menu index is, which is what decides its rules.
///
/// Vanilla expresses this with subclasses (`ResultSlot`, `ArmorSlot`, a plain
/// `Slot`) rather than a tag, but the menu's constructor pins the ranges, so
/// one enum says the same thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    /// Slot 0. Never accepts a placement; its contents come from the recipe.
    Result,
    /// Slots 1..5, the 2x2 grid.
    Craft,
    /// Slots 5..9, helmet through boots. Caps at one item and accepts only
    /// what is equippable in that particular slot.
    Armor(ArmorPiece),
    /// Slots 9..36.
    Main,
    /// Slots 36..45.
    Hotbar,
    /// Slot 45. A plain `Slot` in vanilla — it accepts **anything**, not just
    /// shields, which is why it is not an `Armor` variant.
    Offhand,
    /// A plain `Slot` in a container menu (M90): accepts anything, caps at the
    /// item's own max stack. Every slot of a chest, shulker box, dispenser and
    /// hopper is one — none of those menus has a result or equipment slot.
    Plain,
}

/// The four `ArmorSlot`s, in menu order (`SLOT_IDS` is head-first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorPiece {
    Head,
    Chest,
    Legs,
    Feet,
}

/// The kind of a menu slot, or `None` past the end of the menu.
pub fn slot_kind(slot: usize) -> Option<SlotKind> {
    Some(match slot {
        0 => SlotKind::Result,
        1..=4 => SlotKind::Craft,
        5 => SlotKind::Armor(ArmorPiece::Head),
        6 => SlotKind::Armor(ArmorPiece::Chest),
        7 => SlotKind::Armor(ArmorPiece::Legs),
        8 => SlotKind::Armor(ArmorPiece::Feet),
        9..=35 => SlotKind::Main,
        36..=44 => SlotKind::Hotbar,
        45 => SlotKind::Offhand,
        _ => return None,
    })
}

/// Where a menu slot's 16x16 icon sits, relative to the GUI's top-left.
///
/// Transcribed from `InventoryMenu`'s constructor, which is the only place
/// these numbers exist — there is no data file:
///
/// ```text
/// addResultSlot(owner, 154, 28)                   -> slot 0
/// addCraftingGridSlots(98, 18)                    -> slots 1..5, x + y*2
/// ArmorSlot(..., 8, 8 + i * 18)                   -> slots 5..9
/// addStandardInventorySlots(inventory, 8, 84)     -> slots 9..45
/// Slot(inventory, 40, 77, 62)                     -> slot 45
/// ```
///
/// `addStandardInventorySlots` splits into three rows of nine from the given
/// top, then the hotbar at `top + 58` — the 58 is a named local in vanilla
/// (`topToHotbar`), not a coincidence of 3*18 + 4.
/// M87 made this layout-driven: the numbers above now live in
/// [`crate::menu_layout::PLAYER`], expressed as the same `addSlot` blocks every
/// other menu uses, so the player's menu stops being a special case and becomes
/// the one with no protocol id. The behaviour is unchanged and
/// `layout_matches_the_hand_written_positions` proves it against a frozen copy
/// of the original hard-coded match — not against the layout, which would only
/// assert that the implementation equals itself.
pub fn slot_position(slot: usize) -> Option<(i32, i32)> {
    crate::menu_layout::PLAYER
        .position(slot)
        .map(|(x, y)| (x as i32, y as i32))
}

/// `AbstractContainerScreen.isHovering` — **an 18x18 box, not 16x16**.
///
/// The slot's icon is 16 px, but the test is `x >= left - 1 && x < left + w + 1`
/// with `w = 16`, so it reaches one pixel out on every side and the slots tile
/// without gaps. Using the icon's own rect instead leaves a one-pixel dead
/// cross between every pair of neighbours.
pub fn slot_contains(slot: usize, gui_x: f64, gui_y: f64) -> bool {
    crate::menu_layout::PLAYER.slot_contains(slot, gui_x, gui_y)
}

/// The menu slot under a GUI-relative point, or `None`.
///
/// The boxes overlap by their one-pixel bleed, and vanilla's `getHoveredSlot`
/// returns the **first** match in menu order, so this iterates rather than
/// computing an index.
pub fn slot_at(gui_x: f64, gui_y: f64) -> Option<usize> {
    crate::menu_layout::PLAYER.slot_at(gui_x, gui_y)
}

/// `Container.getMaxStackSize()` as seen through a slot — `ArmorSlot`
/// overrides it to 1, everything else inherits the container's 64.
pub fn slot_max_stack(kind: SlotKind) -> i32 {
    match kind {
        SlotKind::Armor(_) => 1,
        _ => rewo_data::item_props_table::DEFAULT_MAX_STACK,
    }
}

/// What an item is, as far as the click arithmetic is concerned.
///
/// A separate input rather than a lookup inside [`Inventory`] because both
/// facts come from generated tables keyed by *name*, and only the caller holds
/// the registry that turns a protocol id into one. An id this build cannot
/// resolve yields `None`, and the click declines to predict rather than
/// guessing a stack cap — the same "nothing rather than something wrong" rule
/// the item render path uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemProps {
    /// `stack.getMaxStackSize()`.
    pub max_stack: i32,
    /// The armour slot this item can be equipped into, if any.
    pub equips: Option<ArmorPiece>,
    /// `FuelValues.isFuel` (M91).
    pub is_fuel: bool,
    /// `canSmelt` per furnace, indexed by [`furnace_index`] — **blast,
    /// furnace, smoker**, which is `minecraft:menu` id order (10, 14, 22).
    ///
    /// Three booleans rather than one because the three furnaces have
    /// different accepted-input sets: a smoker takes food and not ore, a blast
    /// furnace the reverse. One flag would route beef into a blast furnace.
    pub smeltable: [bool; 3],
}

/// Which entry of [`ItemProps::smeltable`] a `minecraft:menu` id selects.
pub fn furnace_index(protocol_id: i32) -> Option<usize> {
    match protocol_id {
        10 => Some(0), // blast_furnace
        14 => Some(1), // furnace
        22 => Some(2), // smoker
        _ => None,
    }
}

/// One slot's new contents, as predicted locally.
pub type SlotChange = (u16, Option<ItemSlot>);

/// What a click did, ready to be applied locally and sent.
#[derive(Clone, Debug, PartialEq)]
pub struct ClickPrediction {
    /// The menu slot clicked.
    pub slot: i16,
    /// `buttonNum` — 0 primary, 1 secondary.
    pub button: i8,
    /// Every slot whose contents changed, in menu order.
    pub changed: Vec<SlotChange>,
    /// The cursor stack afterwards.
    pub carried: Option<ItemSlot>,
}

impl Inventory {
    /// `ItemStack.isSameItemSameComponents` (M41 — exact).
    ///
    /// The item ids must match and the patches must agree. M35 could only ask
    /// "does either side carry components at all", because nothing decoded
    /// them; every patched stack therefore swapped rather than merging, and
    /// two identically-enchanted books would not stack. Now the patch is
    /// walked and digested, so equal patches compare equal.
    ///
    /// The remaining error is a digest collision, which would merge two stacks
    /// vanilla keeps apart — the direction M35's approximation was written to
    /// avoid. At 64 bits over the few dozen stacks in one inventory that is
    /// far less likely than the approximation it replaces was to be wrong.
    pub fn same_item_same_components(a: ItemSlot, b: ItemSlot) -> bool {
        a.item_id == b.item_id && a.components == b.components
    }

    /// `Slot.mayPlace`.
    fn may_place(kind: SlotKind, props: ItemProps) -> bool {
        match kind {
            // `ResultSlot.mayPlace` is `return false`.
            SlotKind::Result => false,
            // `ArmorSlot.mayPlace` is `owner.isEquippableInSlot(stack, slot)`,
            // and an item with no equippable component is main-hand-only — so
            // absence refuses, it does not default to allowed.
            SlotKind::Armor(piece) => props.equips == Some(piece),
            _ => true,
        }
    }

    /// `Slot.allowModification` — `mayPickup && mayPlace(getItem())`.
    ///
    /// Only the result slot ever answers false, and only because it refuses
    /// every placement including its own contents. It is what stops a partial
    /// take out of the crafting output.
    fn allow_modification(kind: SlotKind, occupant: ItemProps) -> bool {
        Self::may_place(kind, occupant)
    }

    /// `AbstractContainerMenu.doClick`'s PICKUP branch, for buttons 0 and 1.
    ///
    /// Returns the prediction **without applying it** — the caller applies and
    /// sends, in that order, so a send that fails cannot leave the local
    /// inventory ahead of the server's.
    ///
    /// `props` resolves an item id; `None` means this build does not know the
    /// item, and the whole click is declined rather than predicted against a
    /// guessed cap.
    ///
    /// Only `ContainerInput.PICKUP` is implemented. Quick-move (shift-click),
    /// swap (number keys), throw (Q), clone and quick-craft (drag) each have
    /// their own arm in vanilla and none is reproduced here; the caller must
    /// not send them, because an unpredicted `changedSlots` map would be
    /// rejected wholesale.
    pub fn click_pickup(
        &self,
        slot: i32,
        button: i8,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Option<ClickPrediction> {
        if button != 0 && button != 1 {
            return None;
        }
        let primary = button == 0;
        let mut changed: Vec<SlotChange> = Vec::new();
        let mut carried = self.carried;

        // `slotIndex == -999` is a click outside the window, which drops the
        // carried stack. Rewo does not model its own click spawning a dropped
        // item entity, so it declines rather than predicting a stack that
        // vanishes into nothing.
        let index = usize::try_from(slot).ok()?;
        let kind = self.layout.slot_kind(index)?;
        let clicked = self.slots[index];

        // Resolve every stack this click touches up front: a click that cannot
        // be predicted must change nothing at all, not stop half way.
        let clicked_props = match clicked {
            Some(s) => Some(props(s.item_id)?),
            None => None,
        };
        let carried_props = match carried {
            Some(s) => Some(props(s.item_id)?),
            None => None,
        };
        let cap = |p: ItemProps| slot_max_stack(kind).min(p.max_stack);

        match (clicked, carried) {
            // Empty slot, something on the cursor: insert as much as fits.
            (None, Some(mut held)) => {
                let hp = carried_props?;
                if Self::may_place(kind, hp) {
                    let amount = if primary { held.count } else { 1 };
                    // `safeInsert`: min(inputAmount, stack count, headroom).
                    let n = amount.min(held.count).min(cap(hp));
                    if n > 0 {
                        changed.push((index as u16, Some(ItemSlot { count: n, ..held })));
                        held.count -= n;
                        carried = (held.count > 0).then_some(held);
                    }
                }
            }
            // Occupied slot, empty cursor: take all, or half rounded up.
            (Some(stack), None) => {
                // `tryRemove(amount, Integer.MAX_VALUE, player)` — that huge
                // maxAmount is why `allowModification` never bites here, and
                // why a crafting result can be taken whole but not in part.
                let amount = if primary { stack.count } else { (stack.count + 1) / 2 };
                if amount > 0 {
                    let left = stack.count - amount;
                    changed.push((
                        index as u16,
                        (left > 0).then(|| ItemSlot { count: left, ..stack }),
                    ));
                    carried = Some(ItemSlot { count: amount, ..stack });
                }
            }
            (Some(stack), Some(mut held)) => {
                let hp = carried_props?;
                let cp = clicked_props?;
                if Self::may_place(kind, hp) {
                    if Self::same_item_same_components(stack, held) {
                        // Merge into the slot, up to its cap.
                        let amount = if primary { held.count } else { 1 };
                        let n = amount.min(held.count).min(cap(hp) - stack.count);
                        if n > 0 {
                            changed.push((
                                index as u16,
                                Some(ItemSlot { count: stack.count + n, ..stack }),
                            ));
                            held.count -= n;
                            carried = (held.count > 0).then_some(held);
                        }
                    } else if held.count <= cap(hp) {
                        // Swap. That guard is why a 64-stack cannot be dropped
                        // into an armour slot by swapping the helmet out.
                        changed.push((index as u16, Some(held)));
                        carried = Some(stack);
                    }
                } else if Self::same_item_same_components(stack, held) {
                    // The slot refuses the placement but holds the same item —
                    // take from it onto the cursor instead, which is how a
                    // crafting result is collected onto a partial stack.
                    //
                    // `tryRemove(count, carriedMax - carriedCount, player)`:
                    // the second argument is a *maxAmount*, and when it is
                    // below the slot's count `allowModification` decides
                    // whether a partial take is allowed at all.
                    let headroom = hp.max_stack - held.count;
                    let amount = stack.count.min(headroom);
                    let partial = headroom < stack.count;
                    if amount > 0 && (!partial || Self::allow_modification(kind, cp)) {
                        let left = stack.count - amount;
                        changed.push((
                            index as u16,
                            (left > 0).then(|| ItemSlot { count: left, ..stack }),
                        ));
                        held.count += amount;
                        carried = Some(held);
                    }
                }
            }
            (None, None) => {}
        }

        Some(ClickPrediction {
            slot: slot as i16,
            button,
            changed,
            carried,
        })
    }

    /// Apply a prediction locally, so the screen responds without waiting for
    /// the round trip. The server either agrees, or corrects with a
    /// `container_set_content` that overwrites all of it.
    pub fn apply_prediction(&mut self, p: &ClickPrediction) {
        for &(slot, value) in &p.changed {
            if let Some(s) = self.slots.get_mut(slot as usize) {
                *s = value;
            }
        }
        self.carried = p.carried;
    }
}

/// `ContainerInput.QUICK_MOVE`'s wire id — shift-click.
pub const CONTAINER_INPUT_QUICK_MOVE: i32 = 1;

impl Inventory {
    /// `ItemStack.isStackable()` — `getMaxStackSize() > 1 && !isDamaged()`
    /// (M41 — exact).
    ///
    /// `isDamaged` is `damage > 0`, which the patch now carries. M35 read it
    /// as "carries any component", which made an item with, say, a custom name
    /// unstackable when vanilla stacks it happily.
    fn is_stackable(stack: ItemSlot, props: ItemProps) -> bool {
        props.max_stack > 1 && stack.damage.unwrap_or(0) <= 0
    }

    /// `AbstractContainerMenu.moveItemStackTo` — two passes over a slot range.
    ///
    /// The first merges into slots already holding the same item; the second
    /// places the remainder into the first empty slot that will take it. The
    /// asymmetry between them is easy to miss: the merge pass runs to the end
    /// of the range, but the placement pass **stops after one slot**, so a
    /// stack too large for one empty slot leaves the rest behind rather than
    /// spreading across several.
    ///
    /// `backwards` walks the range from its top, which the crafting result
    /// uses so a craft fills the hotbar from the right.
    fn move_stack_to(
        &self,
        slots: &mut [Option<ItemSlot>],
        moving: &mut ItemSlot,
        range: std::ops::Range<usize>,
        backwards: bool,
        props: &dyn Fn(i32) -> Option<ItemProps>,
        changed: &mut Vec<SlotChange>,
    ) -> Option<bool> {
        let mut any = false;
        let p = props(moving.item_id)?;
        let order: Vec<usize> = if backwards {
            range.clone().rev().collect()
        } else {
            range.clone().collect()
        };

        if Self::is_stackable(*moving, p) {
            for &i in &order {
                if moving.count == 0 {
                    break;
                }
                let Some(target) = slots[i] else { continue };
                if !Self::same_item_same_components(*moving, target) {
                    continue;
                }
                let kind = self.layout.slot_kind(i)?;
                let cap = slot_max_stack(kind).min(props(target.item_id)?.max_stack);
                let total = target.count + moving.count;
                if total <= cap {
                    moving.count = 0;
                    slots[i] = Some(ItemSlot { count: total, ..target });
                    changed.push((i as u16, slots[i]));
                    any = true;
                } else if target.count < cap {
                    moving.count -= cap - target.count;
                    slots[i] = Some(ItemSlot { count: cap, ..target });
                    changed.push((i as u16, slots[i]));
                    any = true;
                }
            }
        }

        if moving.count > 0 {
            for &i in &order {
                if slots[i].is_some() {
                    continue;
                }
                let kind = self.layout.slot_kind(i)?;
                if !Self::may_place(kind, p) {
                    continue;
                }
                let cap = slot_max_stack(kind).min(p.max_stack);
                let n = moving.count.min(cap);
                slots[i] = Some(ItemSlot { count: n, ..*moving });
                changed.push((i as u16, slots[i]));
                moving.count -= n;
                any = true;
                // Vanilla breaks here — one empty slot only.
                break;
            }
        }
        Some(any)
    }

    /// `InventoryMenu.quickMoveStack` — where a shift-clicked stack goes.
    ///
    /// The routing is not "the other half of the inventory": armour and the
    /// off-hand are checked *first* for an item that fits them and whose slot
    /// is empty, which is why shift-clicking a helmet equips it rather than
    /// moving it to the hotbar.
    /// Where a shift-click from `slot` sends its stack, and whether the
    /// destination range fills backwards.
    ///
    /// `quickMoveStack` is a per-menu-class **override** in vanilla, so this
    /// dispatches on the menu's shape rather than computing one (M90). An
    /// untranscribed menu returns `None`, which the caller turns into "not
    /// predictable — not sent": moving nothing is inert, where sending a
    /// shift-click under another menu's rules moves the wrong stack to the
    /// wrong place and the server applies it.
    fn quick_move_destination(
        &self,
        slot: usize,
        item: ItemSlot,
        slots: &[Option<ItemSlot>],
        p: ItemProps,
    ) -> Option<(std::ops::Range<usize>, bool)> {
        use crate::menu_layout::QuickMove;
        match self.layout.quick_move() {
            QuickMove::Unimplemented => return None,
            QuickMove::SimpleContainer { container_slots } => {
                // ChestMenu / ShulkerBoxMenu / DispenserMenu / HopperMenu:
                //
                //   if (slotIndex < containerSize)
                //       moveItemStackTo(stack, containerSize, slots.size(), true);
                //   else
                //       moveItemStackTo(stack, 0, containerSize, false);
                //
                // Container to player fills BACKWARDS — from the top of the
                // range, which is the hotbar's right-hand end, because
                // `addStandardInventorySlots` appends the hotbar last.
                return Some(if slot < container_slots {
                    (container_slots..slots.len(), true)
                } else {
                    (0..container_slots, false)
                });
            }
            QuickMove::Furnace => {
                // AbstractFurnaceMenu.quickMoveStack. The literal ranges are
                // 3, 30 and 39 in vanilla; a furnace is 3 container slots plus
                // the player's 36, so they are container / container+27 / end.
                let (ingredient, fuel, result) = (0usize, 1usize, 2usize);
                let player = 3usize;
                let hotbar = player + 27;
                let end = slots.len();
                return Some(if slot == result {
                    // The result fills the player backwards.
                    (player..end, true)
                } else if slot == ingredient || slot == fuel {
                    (player..end, false)
                } else {
                    // A player slot. canSmelt FIRST, then isFuel — a log is
                    // both, and vanilla sends it to the ingredient slot.
                    let _ = item;
                    // The menu decides WHICH accepted-input set applies; an
                    // id this client cannot resolve never reaches here,
                    // because `props` already returned None for it.
                    let Some(which) = furnace_index(self.layout.protocol_id) else {
                        return None;
                    };
                    if p.smeltable[which] {
                        (ingredient..ingredient + 1, false)
                    } else if p.is_fuel {
                        (fuel..fuel + 1, false)
                    } else if slot < hotbar {
                        (hotbar..end, false)
                    } else {
                        (player..hotbar, false)
                    }
                });
            }
            QuickMove::PlayerInventory => {}
        }
        Some(match slot {
            // The crafting result fills backwards, so a craft lands in the
            // hotbar's right-hand slots first.
            0 => (9..45, true),
            1..=8 => (9..45, false),
            _ => {
                // Armour first: `8 - eqSlot.getIndex()` is the menu slot, and
                // it only applies when that slot is empty.
                if let Some(piece) = p.equips {
                    let armour = ARMOR_MENU_START
                        + match piece {
                            ArmorPiece::Head => 0,
                            ArmorPiece::Chest => 1,
                            ArmorPiece::Legs => 2,
                            ArmorPiece::Feet => 3,
                        };
                    if slots[armour].is_none() {
                        return Some((armour..armour + 1, false));
                    }
                }
                let _ = item;
                match slot {
                    9..=35 => (36..45, false),
                    36..=44 => (9..36, false),
                    _ => (9..45, false),
                }
            }
        })
    }

    /// `doClick`'s `QUICK_MOVE` arm — shift-click.
    ///
    /// The outer loop is vanilla's:
    ///
    /// ```text
    /// clicked = quickMoveStack(player, slotIndex);
    /// while (!clicked.isEmpty() && isSameItem(slot.getItem(), clicked))
    ///     clicked = quickMoveStack(player, slotIndex);
    /// ```
    ///
    /// which repeats until the source slot is empty or stops changing — a
    /// single call moves at most one destination slot's worth, so a full stack
    /// spread across several empty slots needs several passes.
    pub fn click_quick_move(
        &self,
        slot: i32,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Option<ClickPrediction> {
        let index = usize::try_from(slot).ok()?;
        // A bounds check against THIS menu, not the player's. It used to be
        // `self.layout.slot_kind(index)?`, whose result was then discarded (`let _ = kind`)
        // — so it was only ever a bounds check, and a 46-slot one: a chest's
        // slots 46 and up returned `None` and silently moved nothing, while
        // 0..46 fell through to the player's routing and silently moved the
        // wrong stack.
        if index >= self.slots.len() {
            return None;
        }
        let source = self.slots[index]?;
        let p = props(source.item_id)?;

        let mut slots = self.slots.clone();
        let mut changed: Vec<SlotChange> = Vec::new();
        // Bounded rather than `loop`: each pass either empties the source or
        // fills one destination, so the menu's own slot count is the ceiling.
        for _ in 0..slots.len() {
            let Some(current) = slots[index] else { break };
            let mut moving = current;
            let (range, backwards) = self.quick_move_destination(index, moving, &slots, p)?;
            let moved = self.move_stack_to(
                &mut slots, &mut moving, range, backwards, props, &mut changed,
            )?;
            if !moved {
                break;
            }
            slots[index] = (moving.count > 0).then_some(moving);
            changed.push((index as u16, slots[index]));
            if moving.count == 0 {
                break;
            }
        }
        if changed.is_empty() {
            return None;
        }
        // One entry per slot, last write wins — the wire carries a map, and a
        // slot touched twice by two passes must appear once.
        let mut seen: Vec<SlotChange> = Vec::new();
        for (s, v) in changed.into_iter().rev() {
            if !seen.iter().any(|(t, _)| *t == s) {
                seen.push((s, v));
            }
        }
        seen.sort_by_key(|(s, _)| *s);
        Some(ClickPrediction {
            slot: slot as i16,
            button: 0,
            changed: seen,
            carried: self.carried,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original hard-coded `slot_position`, frozen at the commit before
    /// M87 made it layout-driven.
    ///
    /// This is the refactor's oracle and it is deliberately a *copy*, not a
    /// call: asserting the new implementation against `menu_layout::PLAYER`
    /// would only prove the layout equals itself, which is M41's `t4` and
    /// M59's recorded failure mode. Transcribed from `InventoryMenu`'s
    /// constructor; if a future version moves a slot, this and the layout
    /// disagree and the test says which.
    fn legacy_slot_position(slot: usize) -> Option<(i32, i32)> {
        Some(match slot {
            0 => (154, 28),
            1..=4 => {
                let i = (slot - 1) as i32;
                (98 + (i % 2) * 18, 18 + (i / 2) * 18)
            }
            5..=8 => (8, 8 + (slot - 5) as i32 * 18),
            9..=35 => {
                let i = (slot - 9) as i32;
                (8 + (i % 9) * 18, 84 + (i / 9) * 18)
            }
            36..=44 => (8 + (slot - 36) as i32 * 18, 84 + 58),
            45 => (77, 62),
            _ => return None,
        })
    }

    // -- M90: shift-click routes by the menu's own quickMoveStack ----------

    fn chest_with(slot: usize, item: Option<ItemSlot>) -> Inventory {
        // generic_9x3: 27 container slots then the player's 36.
        let mut c = Inventory::with_layout(crate::menu_layout::layout_of(2).unwrap());
        let mut v = vec![None; c.slot_count()];
        v[slot] = item;
        assert!(c.set_content(1, &v, None));
        c
    }

    fn plain_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            max_stack: 64,
            equips: None,
            is_fuel: false,
            smeltable: [false; 3],
        })
    }

    /// An item that is BOTH fuel and smeltable, which is what a log is.
    fn log_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            max_stack: 64,
            equips: None,
            is_fuel: true,
            smeltable: [false, true, false], // smeltable in a furnace only
        })
    }

    /// Fuel that is not smeltable, which is what coal is.
    fn coal_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            max_stack: 64,
            equips: None,
            is_fuel: true,
            smeltable: [false; 3],
        })
    }

    #[test]
    fn a_chests_container_slot_shift_clicks_into_the_player_range() {
        let c = chest_with(0, stack(1, 5));
        let p = c.click_quick_move(0, &plain_props).expect("predictable");
        // Everything it touched is at or past the container's 27 slots.
        assert!(!p.changed.is_empty());
        // The source slot is in `changed` too — emptied to None — so the
        // claim is about every OTHER touched slot. (This assertion was written
        // without that and failed on it; the routing was right.)
        assert!(
            p.changed.iter().all(|&(s, _)| s == 0 || s as usize >= 27),
            "a container slot must move into the player's range, got {:?}",
            p.changed
        );
        assert_eq!(p.changed.iter().find(|&&(s, _)| s == 0).unwrap().1, None);
    }

    #[test]
    fn a_chests_player_slot_shift_clicks_into_the_container_range() {
        let c = chest_with(30, stack(1, 5));
        let p = c.click_quick_move(30, &plain_props).expect("predictable");
        assert!(!p.changed.is_empty());
        assert!(
            p.changed.iter().all(|&(s, _)| (s as usize) < 27 || s as usize == 30),
            "a player slot must move into the container, got {:?}",
            p.changed
        );
    }

    #[test]
    fn a_chest_slot_past_the_players_46_is_no_longer_inert() {
        // The bug this replaced: `self.layout.slot_kind(index)?` returned None past 45, so
        // slots 46..63 of a chest silently moved nothing at all. 54 is the
        // chest menu's first hotbar slot.
        let c = chest_with(54, stack(1, 5));
        assert!(
            c.click_quick_move(54, &plain_props).is_some(),
            "slot 54 must be predictable — it was None before M90"
        );
    }

    #[test]
    fn a_container_fills_the_player_from_the_top_of_the_range() {
        // ChestMenu passes `true` for reverse, so the first empty slot taken
        // is the LAST — the hotbar's right-hand end, since
        // addStandardInventorySlots appends the hotbar after the main rows.
        let c = chest_with(0, stack(1, 5));
        let p = c.click_quick_move(0, &plain_props).unwrap();
        let placed = p.changed.iter().map(|&(s, _)| s as usize).max().unwrap();
        assert_eq!(placed, 62, "the last slot of a 63-slot chest menu");
    }

    // -- M91: the furnace shape --------------------------------------------

    /// A furnace (menu 14): ingredient 0, fuel 1, result 2, player 3..39.
    fn furnace_with(slot: usize, item: Option<ItemSlot>) -> Inventory {
        let mut f = Inventory::with_layout(crate::menu_layout::layout_of(14).unwrap());
        let mut v = vec![None; f.slot_count()];
        v[slot] = item;
        assert!(f.set_content(1, &v, None));
        f
    }

    fn moved_into(p: &ClickPrediction, from: usize) -> Vec<usize> {
        p.changed
            .iter()
            .map(|&(s, _)| s as usize)
            .filter(|&s| s != from)
            .collect()
    }

    #[test]
    fn a_log_routes_to_the_ingredient_slot_not_the_fuel_slot() {
        // THE case that decides the shape. Vanilla checks canSmelt BEFORE
        // isFuel, and a log is both — fuel, and smeltable to charcoal — so it
        // goes to the ingredient slot. Routing on isFuel alone puts it in the
        // fuel slot, which is wrong and looks entirely reasonable.
        let f = furnace_with(20, stack(1, 5));
        let p = f.click_quick_move(20, &log_props).expect("predictable");
        assert_eq!(moved_into(&p, 20), vec![0], "the ingredient slot");
    }

    #[test]
    fn coal_routes_to_the_fuel_slot() {
        let f = furnace_with(20, stack(1, 5));
        let p = f.click_quick_move(20, &coal_props).expect("predictable");
        assert_eq!(moved_into(&p, 20), vec![1], "the fuel slot");
    }

    #[test]
    fn a_neither_item_crosses_between_the_players_own_rows() {
        // Not smeltable, not fuel: main (3..30) goes to the hotbar, and the
        // hotbar (30..39) goes to main. Vanilla's last two branches.
        let f = furnace_with(5, stack(1, 5));
        let p = f.click_quick_move(5, &plain_props).expect("predictable");
        assert!(
            moved_into(&p, 5).iter().all(|&s| (30..39).contains(&s)),
            "a main-inventory slot must cross to the hotbar, got {:?}",
            p.changed
        );

        let g = furnace_with(35, stack(1, 5));
        let q = g.click_quick_move(35, &plain_props).expect("predictable");
        assert!(
            moved_into(&q, 35).iter().all(|&s| (3..30).contains(&s)),
            "a hotbar slot must cross to main, got {:?}",
            q.changed
        );
    }

    #[test]
    fn the_result_slot_fills_the_player_backwards() {
        let f = furnace_with(2, stack(1, 5));
        let p = f.click_quick_move(2, &plain_props).expect("predictable");
        assert_eq!(
            moved_into(&p, 2),
            vec![38],
            "the result fills from the top of the player range"
        );
    }

    #[test]
    fn the_ingredient_and_fuel_slots_empty_forwards() {
        // Slots 0 and 1 take the same branch, and it is NOT the result's:
        // forwards, so the first free player slot rather than the last.
        for from in [0usize, 1] {
            let f = furnace_with(from, stack(1, 5));
            let p = f.click_quick_move(from as i32, &plain_props).expect("predictable");
            assert_eq!(moved_into(&p, from), vec![3], "slot {from} fills forwards");
        }
    }

    #[test]
    fn a_smoker_and_a_blast_furnace_read_different_accepted_sets() {
        // `smeltable` is three booleans, not one, because the sets differ. A
        // log is smeltable in a furnace only, so in a smoker it is merely fuel.
        let smoker = Inventory::with_layout(crate::menu_layout::layout_of(22).unwrap());
        let mut v = vec![None; smoker.slot_count()];
        v[20] = stack(1, 5);
        let mut smoker = smoker;
        smoker.set_content(1, &v, None);
        let p = smoker.click_quick_move(20, &log_props).expect("predictable");
        assert_eq!(
            moved_into(&p, 20),
            vec![1],
            "a log in a SMOKER is fuel, not an ingredient"
        );
    }

    #[test]
    fn an_untranscribed_menu_declines_rather_than_borrowing_another_shape() {
        // An anvil's quickMoveStack is its own; routing it as a chest would
        // move the wrong stack and the server would apply it. Declining sends
        // nothing.
        let mut anvil = Inventory::with_layout(crate::menu_layout::layout_of(8).unwrap());
        let mut v = vec![None; anvil.slot_count()];
        v[0] = stack(1, 5);
        anvil.set_content(1, &v, None);
        assert_eq!(
            anvil.layout().quick_move(),
            crate::menu_layout::QuickMove::Unimplemented
        );
        assert!(anvil.click_quick_move(0, &plain_props).is_none());
    }

    #[test]
    fn layout_matches_the_hand_written_positions() {
        for slot in 0..MENU_SLOTS {
            assert_eq!(
                slot_position(slot),
                legacy_slot_position(slot),
                "menu slot {slot} moved"
            );
        }
    }

    #[test]
    fn the_player_layout_ends_where_the_menu_does() {
        assert_eq!(crate::menu_layout::PLAYER.slot_count(), MENU_SLOTS);
        assert_eq!(slot_position(MENU_SLOTS), None);
        assert_eq!(legacy_slot_position(MENU_SLOTS), None);
    }

    #[test]
    fn a_container_layout_sizes_its_own_storage() {
        // The point of the generalization: the same struct is a chest.
        let chest = Inventory::with_layout(crate::menu_layout::layout_of(5).unwrap());
        assert_eq!(chest.slot_count(), 54 + 36, "generic_9x6 is 54 + the player's");
        assert_eq!(chest.layout().name, "generic_9x6");
        // And set_content's length check follows the layout, not a constant.
        assert!(chest_accepts(&chest, 90));
        assert!(!chest_accepts(&chest, MENU_SLOTS));
    }

    fn chest_accepts(proto: &Inventory, n: usize) -> bool {
        let mut c = Inventory::with_layout(proto.layout());
        c.set_content(1, &vec![None; n], None)
    }

    #[test]
    fn the_lectern_is_a_one_slot_menu_and_does_not_underflow() {
        // `slots.len() - 36` is a panic here; nothing may assume a trailing
        // player inventory.
        let lectern = Inventory::with_layout(crate::menu_layout::layout_of(17).unwrap());
        assert_eq!(lectern.slot_count(), 1);
        assert_eq!(lectern.menu_slot(0), None);
        assert_eq!(lectern.menu_slot(1), None, "out of range reads empty");
        assert!(lectern.layout().slot_at(-99.0, -99.0).is_none());
    }

    #[test]
    fn hover_geometry_is_unchanged_for_the_player() {
        // slot_at/slot_contains moved onto the layout; the player's answers
        // must be identical. Sweep the whole GUI rather than spot-checking,
        // since the 18x18 boxes overlap and an off-by-one would only show at
        // a seam.
        for y in 0..GUI_HEIGHT {
            for x in 0..GUI_WIDTH {
                let (fx, fy) = (x as f64, y as f64);
                let expect = (0..MENU_SLOTS).find(|&s| {
                    let Some((l, t)) = legacy_slot_position(s) else {
                        return false;
                    };
                    let (l, t) = (l as f64, t as f64);
                    fx >= l - 1.0 && fx < l + 17.0 && fy >= t - 1.0 && fy < t + 17.0
                });
                assert_eq!(slot_at(fx, fy), expect, "hover differs at ({x}, {y})");
            }
        }
    }

    #[test]
    fn the_player_menu_has_no_protocol_id() {
        // It is never opened by `open_screen`; it is container id 0 and exists
        // for the whole session. So it must not be reachable by registry id --
        // `layout_of(-1)` answering PLAYER would let a malformed packet open
        // the player's own inventory as if it were a container.
        assert_eq!(
            crate::menu_layout::PLAYER.protocol_id,
            crate::menu_layout::NO_PROTOCOL_ID
        );
        assert!(crate::menu_layout::layout_of(crate::menu_layout::NO_PROTOCOL_ID).is_none());
        for (i, m) in crate::menu_layout::REGISTRY.iter().enumerate() {
            assert_ne!(m.name, "player", "PLAYER must not be in the registry (id {i})");
        }
    }

    fn stack(id: i32, n: i32) -> Option<ItemSlot> {
        Some(ItemSlot {
            item_id: id,
            count: n,
            has_components: false,
            components: 0,
            damage: None,
            max_damage: None,
            enchanted: false,
            trim_material: None,
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

    // -- the inventory-index coordinate system (M69) ---------------------

    /// **The reversal witness.** The two armour ranges run against each other:
    /// inventory `36..40` is FEET, LEGS, CHEST, HEAD, while menu `5..9` is
    /// HEAD, CHEST, LEGS, FEET. Transcribed here as four literals from
    /// `InventoryMenu`'s `SLOT_IDS` + `39 - i`, so this fails if the
    /// production arithmetic is ever "simplified" into the plausible
    /// `index - 31`.
    ///
    /// Mutation partner: change the `36..=39` arm to
    /// `Applied(ARMOR_MENU_START + (index - ARMOR_INDEX_START) as usize)` —
    /// every index still maps into the armour range, every write still lands
    /// in a real slot, and boots render on the head.
    #[test]
    fn the_two_armour_ranges_run_in_opposite_directions() {
        // (inventory index, menu slot, piece) — from InventoryMenu's ctor.
        for (index, menu, piece) in [(36, 8, "feet"), (37, 7, "legs"), (38, 6, "chest"), (39, 5, "head")]
        {
            assert_eq!(
                menu_slot_of_inventory_index(index),
                IndexWrite::Applied(menu),
                "inventory index {index} is the {piece} slot, menu slot {menu}"
            );
        }
        // Stated as the property rather than only as the table: the mapping is
        // order-reversing over the armour range.
        let a = menu_slot_of_inventory_index(ARMOR_INDEX_START);
        let b = menu_slot_of_inventory_index(ARMOR_INDEX_START + 3);
        assert!(
            matches!((a, b), (IndexWrite::Applied(x), IndexWrite::Applied(y)) if x > y),
            "the lowest inventory index must map to the HIGHEST menu slot"
        );
        // And `armor(0)` is the helmet in menu space, which is index 39.
        let mut inv = Inventory::default();
        assert_eq!(inv.set_inventory_index(39, stack(7, 1)), IndexWrite::Applied(5));
        assert_eq!(inv.armor(0).unwrap().item_id, 7, "armor(0) is the helmet");
        assert_eq!(inv.armor(3), None, "the boots slot must still be empty");
    }

    /// The whole table, every index vanilla maps, in one place. A range that
    /// silently shifts by one shows up here and nowhere else.
    #[test]
    fn every_mapped_inventory_index_lands_where_vanilla_puts_it() {
        for i in 0..9 {
            assert_eq!(
                menu_slot_of_inventory_index(i),
                IndexWrite::Applied(HOTBAR_MENU_START + i as usize),
                "hotbar index {i}"
            );
        }
        // The main inventory is the identity — both `addInventoryExtendedSlots`
        // and `Inventory.items` number it `x + (y+1)*9`.
        for i in 9..INVENTORY_ITEMS_SIZE {
            assert_eq!(
                menu_slot_of_inventory_index(i),
                IndexWrite::Applied(i as usize),
                "main inventory index {i} must be its own menu slot"
            );
        }
        assert_eq!(
            menu_slot_of_inventory_index(OFFHAND_INDEX),
            IndexWrite::Applied(OFFHAND_MENU_SLOT)
        );
        // Every mapped index lands somewhere distinct, and never on the
        // crafting slots — `InventoryMenu` backs menu 0..5 with its own
        // `CraftingContainer`, so no inventory index reaches them.
        let mut seen = std::collections::HashSet::new();
        for i in 0..=OFFHAND_INDEX {
            if let IndexWrite::Applied(slot) = menu_slot_of_inventory_index(i) {
                assert!(slot >= ARMOR_MENU_START, "index {i} reached crafting slot {slot}");
                assert!(seen.insert(slot), "menu slot {slot} claimed twice");
            }
        }
        assert_eq!(seen.len(), 41, "9 hotbar + 27 main + 4 armour + 1 off-hand");
    }

    /// Body armour and the saddle are real `EntityEquipment` slots with no
    /// `InventoryMenu` counterpart, and that is a **third** state — not the
    /// same as an index vanilla does not map at all. Collapsing the two would
    /// make "Rewo has nowhere to put this" indistinguishable from "the server
    /// sent nonsense".
    #[test]
    fn body_armour_and_saddle_have_no_menu_slot_and_that_is_not_out_of_range() {
        for i in [BODY_ARMOR_INDEX, SADDLE_INDEX] {
            assert_eq!(menu_slot_of_inventory_index(i), IndexWrite::NoMenuSlot);
        }
        for i in [-1, 43, 45, 46, 100, i32::MIN, i32::MAX] {
            assert_eq!(
                menu_slot_of_inventory_index(i),
                IndexWrite::OutOfRange,
                "{i} is outside every index Inventory.setItem maps"
            );
        }
        // Neither writes anything.
        let mut inv = Inventory::default();
        assert_eq!(inv.set_inventory_index(BODY_ARMOR_INDEX, stack(1, 1)), IndexWrite::NoMenuSlot);
        assert_eq!(inv.set_inventory_index(-1, stack(1, 1)), IndexWrite::OutOfRange);
        assert!(inv.is_empty(), "neither may touch a menu slot");
    }

    /// **The state-id witness.** `set_player_inventory` carries no state id;
    /// `Inventory.setItem` never touches the container menu. Advancing it here
    /// would make the next click echo a number the server never issued, and a
    /// stale state id is exactly what triggers the full resync this packet
    /// exists to avoid.
    ///
    /// Mutation partner: have `set_inventory_index` call `set_slot` instead —
    /// the item lands in the same place and this fails.
    #[test]
    fn an_index_write_leaves_the_state_id_and_the_update_count_alone() {
        let mut inv = Inventory::default();
        assert!(inv.set_content(77, &[None; MENU_SLOTS], None));
        assert_eq!(inv.state_id(), 77);
        assert_eq!(inv.content_updates(), 1);

        assert_eq!(inv.set_inventory_index(0, stack(5, 3)), IndexWrite::Applied(36));
        assert_eq!(inv.held().unwrap().item_id, 5, "hotbar index 0 is the held slot");
        assert_eq!(inv.state_id(), 77, "an index write is not a menu update");
        assert_eq!(inv.content_updates(), 1, "and it is not a resync either");

        // Its sibling, by contrast, does carry one.
        assert!(inv.set_slot(78, 36, stack(6, 1)));
        assert_eq!(inv.state_id(), 78);
    }

    /// The cursor's authoritative write. `set_carried` replaces it outright,
    /// including with nothing — a server clearing the cursor sends an empty
    /// stack, and treating that as "no change" would leave a phantom stack on
    /// the pointer.
    #[test]
    fn the_carried_stack_can_be_set_and_cleared() {
        let mut inv = Inventory::default();
        assert_eq!(inv.carried(), None);
        inv.set_carried(stack(4, 12));
        assert_eq!(inv.carried().unwrap().count, 12);
        assert!(!inv.is_empty(), "a carried stack alone is not an empty inventory");
        inv.set_carried(None);
        assert_eq!(inv.carried(), None);
        assert!(inv.is_empty());
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
        assert_eq!(inv.held().unwrap(), stack(64, 3).unwrap());
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

/// `ContainerInput.SWAP` — a number key, or F for the off-hand.
pub const CONTAINER_INPUT_SWAP: i32 = 2;
/// `ContainerInput.THROW` — Q.
pub const CONTAINER_INPUT_THROW: i32 = 4;
/// `ContainerInput.PICKUP_ALL` — a double click.
pub const CONTAINER_INPUT_PICKUP_ALL: i32 = 6;
/// `Inventory.SLOT_OFFHAND` — the button `SWAP` uses for the off-hand.
pub const SWAP_OFFHAND_BUTTON: i32 = 40;

impl Inventory {
    /// `doClick`'s `SWAP` arm — a number key, or F for the off-hand.
    ///
    /// **Its button is a third coordinate system.** `slotIndex` is a menu slot,
    /// as everywhere else, but `buttonNum` indexes `player.getInventory()`
    /// directly — `0..9` for the hotbar and the literal `40` for the off-hand.
    /// So pressing `1` over the helmet slot is `slot = 5, button = 0`, and the
    /// two numbers are counted from different origins.
    ///
    /// Vanilla's guard is `buttonNum >= 0 && buttonNum < 9 || buttonNum == 40`,
    /// which is not a range check — 9 through 39 are rejected outright rather
    /// than clamped.
    pub fn click_swap(
        &self,
        slot: i32,
        button: i32,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Option<ClickPrediction> {
        if !((0..HOTBAR_SIZE as i32).contains(&button) || button == SWAP_OFFHAND_BUTTON) {
            return None;
        }
        let target_index = usize::try_from(slot).ok()?;
        let kind = self.layout.slot_kind(target_index)?;
        // The inventory index the button names, back in menu coordinates.
        let source_index = if button == SWAP_OFFHAND_BUTTON {
            OFFHAND_MENU_SLOT
        } else {
            HOTBAR_MENU_START + button as usize
        };
        // Swapping a slot with itself is a no-op in vanilla too (the source
        // and target are the same `ItemStack`), and predicting it would emit a
        // changed-slot map for a click that changed nothing.
        if source_index == target_index {
            return None;
        }
        let source = self.slots[source_index];
        let target = self.slots[target_index];
        let mut changed: Vec<SlotChange> = Vec::new();

        match (source, target) {
            (None, None) => {}
            // The target's stack goes to the hotbar. `mayPickup` is only false
            // for slots Rewo does not model, so this is unconditional here.
            (None, Some(t)) => {
                changed.push((source_index as u16, Some(t)));
                changed.push((target_index as u16, None));
            }
            (Some(s), None) => {
                let p = props(s.item_id)?;
                if !Self::may_place(kind, p) {
                    return Some(ClickPrediction {
                        slot: slot as i16,
                        button: button as i8,
                        changed,
                        carried: self.carried,
                    });
                }
                let cap = slot_max_stack(kind).min(p.max_stack);
                if s.count > cap {
                    // `source.split(maxStackSize)` — the hotbar keeps the rest.
                    changed.push((target_index as u16, Some(ItemSlot { count: cap, ..s })));
                    changed.push((
                        source_index as u16,
                        Some(ItemSlot { count: s.count - cap, ..s }),
                    ));
                } else {
                    changed.push((target_index as u16, Some(s)));
                    changed.push((source_index as u16, None));
                }
            }
            (Some(s), Some(t)) => {
                let p = props(s.item_id)?;
                if !Self::may_place(kind, p) {
                    return Some(ClickPrediction {
                        slot: slot as i16,
                        button: button as i8,
                        changed,
                        carried: self.carried,
                    });
                }
                let cap = slot_max_stack(kind).min(p.max_stack);
                if s.count > cap {
                    // The displaced stack goes through `inventory.add`, and if
                    // that fails, `player.drop` — a search Rewo does not model
                    // and a dropped entity it cannot predict. Declining is the
                    // honest answer; the case needs a source stack larger than
                    // the target's cap, so in the player menu it is an armour
                    // slot with a stack of more than one armour piece.
                    return None;
                }
                changed.push((source_index as u16, Some(t)));
                changed.push((target_index as u16, Some(s)));
            }
        }
        if changed.is_empty() {
            return None;
        }
        changed.sort_by_key(|&(s, _)| s);
        Some(ClickPrediction {
            slot: slot as i16,
            button: button as i8,
            changed,
            carried: self.carried,
        })
    }

    /// `doClick`'s `THROW` arm — Q drops one, Ctrl+Q the whole stack.
    ///
    /// Gated on the cursor being **empty**: with something on the cursor, Q
    /// does nothing at all rather than dropping what you are holding.
    ///
    /// The trailing `while` loop in vanilla never runs a second time. With
    /// `button == 1` the amount is the slot's whole count, so the first
    /// `safeTake` empties it and `isSameItem(slot.getItem(), …)` compares
    /// against an empty stack. It reads like a repeat and is a no-op.
    ///
    /// The dropped entity is the server's business — Rewo predicts the slot
    /// and nothing else, which is exactly the part the `changedSlots` map
    /// carries.
    pub fn click_throw(
        &self,
        slot: i32,
        button: i8,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Option<ClickPrediction> {
        if self.carried.is_some() {
            return None;
        }
        let index = usize::try_from(slot).ok()?;
        let kind = self.layout.slot_kind(index)?;
        let stack = self.slots[index]?;
        let p = props(stack.item_id)?;
        let amount = if button == 0 { 1 } else { stack.count };
        // `safeTake(amount, Integer.MAX_VALUE, player)` with a partial amount
        // still consults `allowModification`, which is what stops one item
        // being taken out of a crafting result.
        if amount < stack.count && !Self::allow_modification(kind, p) {
            return None;
        }
        let left = stack.count - amount;
        Some(ClickPrediction {
            slot: slot as i16,
            button,
            changed: vec![(
                index as u16,
                (left > 0).then(|| ItemSlot { count: left, ..stack }),
            )],
            carried: self.carried,
        })
    }

    /// `doClick`'s `PICKUP_ALL` arm — the double click that sweeps a stack up.
    ///
    /// Three things about it are easy to get wrong.
    ///
    /// It only runs when the cursor is **full** and the clicked slot is empty
    /// or unpickable — the second click of a double click lands on a slot the
    /// first one just emptied, which is exactly that condition. Firing it on a
    /// full slot would make an ordinary second click hoover up the inventory.
    ///
    /// It runs **two passes**, and the first one skips full stacks
    /// (`pass != 0 || count != maxStackSize`). So a double click gathers the
    /// partial stacks first and only breaks into full ones if it still has
    /// room — which is why it leaves the tidy stacks alone when it can.
    ///
    /// And `canItemQuickReplace(target, carried, true)` passes `ignoreSize`,
    /// so a stack is a candidate on identity alone; the room left on the
    /// cursor is what bounds each take.
    pub fn click_pickup_all(
        &self,
        slot: i32,
        button: i8,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Option<ClickPrediction> {
        let index = usize::try_from(slot).ok()?;
        let kind = self.layout.slot_kind(index)?;
        let mut carried = self.carried?;
        let cp = props(carried.item_id)?;
        let clicked = self.slots[index];
        // `!slot.hasItem() || !slot.mayPickup(player)`.
        if let Some(stack) = clicked {
            if Self::allow_modification(kind, props(stack.item_id)?) {
                return None;
            }
        }
        let max = cp.max_stack;
        let mut slots = self.slots.clone();
        let mut changed: Vec<SlotChange> = Vec::new();
        let n = slots.len();
        let order: Vec<usize> = if button == 0 {
            (0..n).collect()
        } else {
            (0..n).rev().collect()
        };
        for pass in 0..2 {
            for &i in &order {
                if carried.count >= max {
                    break;
                }
                let Some(target) = slots[i] else { continue };
                if !Self::same_item_same_components(carried, target) {
                    continue;
                }
                let tp = props(target.item_id)?;
                if !Self::allow_modification(self.layout.slot_kind(i)?, tp) {
                    continue;
                }
                // Pass 0 leaves full stacks alone.
                if pass == 0 && target.count == tp.max_stack {
                    continue;
                }
                let take = target.count.min(max - carried.count);
                if take <= 0 {
                    continue;
                }
                carried.count += take;
                let left = target.count - take;
                slots[i] = (left > 0).then(|| ItemSlot { count: left, ..target });
                changed.retain(|&(s, _)| s != i as u16);
                changed.push((i as u16, slots[i]));
            }
        }
        if changed.is_empty() {
            return None;
        }
        changed.sort_by_key(|&(s, _)| s);
        Some(ClickPrediction {
            slot: slot as i16,
            button,
            changed,
            carried: Some(carried),
        })
    }
}

/// `ContainerInput.QUICK_CRAFT` — the drag that spreads a stack over slots.
pub const CONTAINER_INPUT_QUICK_CRAFT: i32 = 5;
/// `getQuickcraftType` 0 — spread the stack evenly.
pub const QUICK_CRAFT_SPLIT: i32 = 0;
/// Type 1 — one item into each slot.
pub const QUICK_CRAFT_ONE: i32 = 1;
/// `slotIndex` for the two phases that name no slot.
pub const QUICK_CRAFT_NO_SLOT: i16 = -999;

impl Inventory {
    /// The packed `buttonNum` a quick-craft phase carries.
    ///
    /// **One byte holds two fields**: `type << 2 | header`, read back out by
    /// `getQuickcraftType` (`mask >> 2 & 3`) and `getQuickcraftHeader`
    /// (`mask & 3`). Header 0 begins the drag, 1 adds a slot, 2 ends it. Send
    /// a bare header and the server reads type 0 — a stack meant to go one per
    /// slot would be spread evenly instead.
    pub fn quick_craft_button(kind: i32, header: i32) -> i8 {
        ((kind << 2) | header) as i8
    }

    /// `getQuickCraftPlaceCount` — how much lands in **each** dragged slot.
    ///
    /// Note what it divides: the stack over the **slot count**, floored. Three
    /// items over two slots is one each and one left on the cursor, not two
    /// and one.
    fn quick_craft_place_count(slot_count: usize, kind: i32, stack: ItemSlot, p: ItemProps) -> i32 {
        match kind {
            QUICK_CRAFT_SPLIT => stack.count / slot_count.max(1) as i32,
            QUICK_CRAFT_ONE => 1,
            // Type 2 is the creative clone, which needs `hasInfiniteMaterials`.
            2 => p.max_stack,
            _ => stack.count,
        }
    }

    /// Whether a slot can join the drag — `canItemQuickReplace(slot, carried,
    /// true) && mayPlace`.
    ///
    /// `ignoreSize` is passed **true**, so a slot already holding the same
    /// item qualifies on identity alone and the room left in it is worked out
    /// later. An empty slot always qualifies.
    fn may_drag_into(&self, index: usize, carried: ItemSlot, p: ItemProps) -> bool {
        let Some(kind) = self.layout.slot_kind(index) else {
            return false;
        };
        if !Self::may_place(kind, p) {
            return false;
        }
        match self.slots[index] {
            None => true,
            Some(occupant) => Self::same_item_same_components(carried, occupant),
        }
    }

    /// The slots a drag would actually accept, in the order they were touched.
    ///
    /// Vanilla filters as each one is added (`quickcraftStatus == 1`), so a
    /// slot the drag passed over but could not use never enters the set and
    /// never appears in a packet. The `count > size` test is what stops a drag
    /// from claiming more slots than the stack can feed.
    pub fn quick_craft_accepts(
        &self,
        touched: &[usize],
        kind: i32,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Vec<usize> {
        let Some(carried) = self.carried else {
            return Vec::new();
        };
        let Some(p) = props(carried.item_id) else {
            return Vec::new();
        };
        let mut out: Vec<usize> = Vec::new();
        for &i in touched {
            if out.contains(&i) {
                continue;
            }
            if kind != 2 && carried.count <= out.len() as i32 {
                continue;
            }
            if self.may_drag_into(i, carried, p) {
                out.push(i);
            }
        }
        out
    }

    /// `doClick`'s `QUICK_CRAFT` end phase — what the drag leaves behind.
    ///
    /// `slots` is the accepted set from [`Self::quick_craft_accepts`].
    ///
    /// **A one-slot drag is not a drag.** Vanilla resets the state and
    /// re-dispatches as `PICKUP` with `buttonNum = quickcraftType`, which maps
    /// type 0 to a primary click (place everything) and type 1 to a secondary
    /// one (place one). So a click-and-release inside a single slot behaves
    /// exactly like the click it looks like, and the caller must send it as a
    /// `PICKUP` rather than a quick-craft — [`Self::quick_craft_is_pickup`]
    /// reports that.
    pub fn click_quick_craft(
        &self,
        slots: &[usize],
        kind: i32,
        props: &dyn Fn(i32) -> Option<ItemProps>,
    ) -> Option<ClickPrediction> {
        if slots.len() < 2 {
            return None;
        }
        let source = self.carried?;
        let p = props(source.item_id)?;
        let mut remaining = source.count;
        let mut changed: Vec<SlotChange> = Vec::new();
        for &i in slots {
            let kind_of = self.layout.slot_kind(i)?;
            if !self.may_drag_into(i, source, p) {
                continue;
            }
            if kind != 2 && source.count < slots.len() as i32 {
                continue;
            }
            let carry = self.slots[i].map_or(0, |s| s.count);
            let max = p.max_stack.min(slot_max_stack(kind_of));
            let place = Self::quick_craft_place_count(slots.len(), kind, source, p);
            let new_count = (place + carry).min(max);
            remaining -= new_count - carry;
            changed.push((i as u16, Some(ItemSlot { count: new_count, ..source })));
        }
        if changed.is_empty() {
            return None;
        }
        changed.sort_by_key(|&(s, _)| s);
        Some(ClickPrediction {
            slot: QUICK_CRAFT_NO_SLOT,
            button: Self::quick_craft_button(kind, 2),
            changed,
            // `source.setCount(remaining)` — an exhausted cursor is empty,
            // not a stack of zero.
            carried: (remaining > 0).then(|| ItemSlot { count: remaining, ..source }),
        })
    }

    /// The `PICKUP` button a one-slot drag collapses into, if it is one.
    ///
    /// Type 0 becomes button 0 and type 1 becomes button 1 — the two numbers
    /// happen to line up, which is why vanilla passes `quickcraftType`
    /// straight through as the button.
    pub fn quick_craft_is_pickup(slots: &[usize], kind: i32) -> Option<(usize, i8)> {
        (slots.len() == 1).then(|| (slots[0], kind as i8))
    }
}
