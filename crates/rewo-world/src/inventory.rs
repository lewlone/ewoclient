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
    /// `EnchantmentHelper.hasAnyEnchantments` (M93e) — `minecraft:enchantments`
    /// **or** `minecraft:stored_enchantments` non-empty.
    ///
    /// **Not [`Self::enchanted`]**, and the difference is not academic. That
    /// field is assigned from `hasFoil()`, which M43 proved respects
    /// `hasAnyEnchantments` — `ENCHANTMENTS` **or** `STORED_ENCHANTMENTS`, which
    /// is what a grindstone's `mayPlace` asks and why an enchanted **book**
    /// (whose enchantments are *stored*) is a valid input.
    ///
    /// **It is NOT `isEnchanted()`.** That reads `minecraft:enchantments`
    /// alone and lives on [`SlotText::is_enchanted`]; this field is the wider
    /// predicate. An earlier version of this comment said the two were the same
    /// thing, which made three near-identical flags read as two — the other
    /// being `enchanted`, which is `has_foil()` and agrees with neither
    /// whenever `ENCHANTMENT_GLINT_OVERRIDE` is present (M43).
    pub any_enchantments: bool,
    /// `minecraft:unbreakable`'s presence in the patch (M93e). The prototype
    /// never carries it, so the patch is the whole answer.
    pub unbreakable: bool,
    /// Whether the patch removed `minecraft:dye` (M93g) — the component half
    /// of `isDyeItem`'s conjunction.
    pub dye_removed: bool,
    /// Whether the patch removed `minecraft:provides_banner_patterns` (M93g).
    pub provides_banner_patterns_removed: bool,
    /// `ItemStack.has(DataComponents.MAP_ID)` (M93f). No prototype carries
    /// MAP_ID, so the patch is the whole answer and this is the whole of
    /// `has()`.
    pub has_map_id: bool,
    /// Whether the patch removed `minecraft:damage` or `minecraft:max_damage`
    /// (M93e) — see `StackComponents::damage_component_removed`.
    pub damage_component_removed: bool,
    /// `minecraft:trim`'s material registry id (M49), for picking the icon
    /// variant. The pattern is not here: an item definition's `select` is on
    /// `minecraft:trim_material` alone, so the pattern changes the worn model
    /// and never the icon.
    pub trim_material: Option<i32>,
}

impl ItemSlot {
    /// A stack with **no `DataComponentPatch` at all** — every component-derived
    /// field at its absent value (M93s).
    ///
    /// For a stack Rewo synthesises rather than decodes: a stonecutter's recipe
    /// button draws its result, which never crossed the wire. Spelled out
    /// rather than `Default`-derived so that adding a component field is a
    /// compile error here and a decision, not a silent `false`.
    pub fn plain(item_id: i32, count: i32) -> Self {
        Self {
            item_id,
            count,
            has_components: false,
            components: 0,
            damage: None,
            max_damage: None,
            enchanted: false,
            any_enchantments: false,
            unbreakable: false,
            dye_removed: false,
            provides_banner_patterns_removed: false,
            has_map_id: false,
            damage_component_removed: false,
            trim_material: None,
        }
    }
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
    /// `minecraft:custom_name` — the anvil-given name, as the raw component.
    ///
    /// **Split from [`Self::item_name`], and the split is load-bearing twice.**
    /// `getStyledHoverName` (`ItemStack.java:827-833`) adds ITALIC iff
    /// `has(CUSTOM_NAME)`, and `Inventory.isUsableForCrafting`
    /// (`Inventory.java:145-147`) tests `has(CUSTOM_NAME)` alone — a merged
    /// `custom_name.or(item_name)` can express neither, and the old merged field
    /// made every `item_name`-only stack unusable by the recipe-book solver.
    ///
    /// Unflattened: the decode has no language table (see
    /// [`crate::chat_style::flatten`]), so a `translate` would have shown its
    /// key and a legacy colour code its two characters.
    pub custom_name: Option<rewo_proto::nbt::Nbt>,
    /// `minecraft:item_name` — a *default* name the item may carry. Lower
    /// precedence than [`Self::custom_name`], higher than the translated id,
    /// and it does **not** italicise.
    pub item_name: Option<rewo_proto::nbt::Nbt>,
    /// `minecraft:lore`, one component per line, unflattened.
    pub lore: Vec<rewo_proto::nbt::Nbt>,
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
    ///
    /// So this **destructures** rather than reading `self.x` seven times: adding
    /// a field is then a compile error here, not a silent omission. Adding
    /// `item_name` beside `custom_name` was exactly the change that could have
    /// slipped through — a stack carrying only a patched `item_name` would have
    /// read as textless and been dropped.
    pub fn is_empty(&self) -> bool {
        let SlotText {
            custom_name,
            item_name,
            lore,
            rarity,
            unbreakable,
            enchantments,
            is_enchanted,
            cooldown_group,
        } = self;
        custom_name.is_none()
            && item_name.is_none()
            && lore.is_empty()
            && rarity.is_none()
            && !*unbreakable
            && enchantments.is_empty()
            && !*is_enchanted
            && cooldown_group.is_none()
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

    /// `Inventory.isUsableForCrafting` (M102).
    ///
    /// ```java
    /// return !item.isDamaged() && !item.isEnchanted() && !item.has(CUSTOM_NAME);
    /// ```
    ///
    /// The recipe book's contents come through `accountSimpleStack`, which
    /// gates on this — so a chipped pickaxe, an enchanted book or a renamed
    /// stack **does not count toward what you can craft**, even though it is
    /// sitting in your inventory. M96 named this predicate in a comment and
    /// applied nothing, so every stack counted.
    ///
    /// It lives on `Inventory` rather than on [`ItemSlot`] because two of its
    /// three inputs are on the text side: only the damage travels with the
    /// stack.
    ///
    /// **`isEnchanted()` is `ENCHANTMENTS` alone**, and the field to read is
    /// [`SlotText::is_enchanted`] — *not* `ItemSlot::any_enchantments`, which
    /// is `hasAnyEnchantments` (enchantments **or** stored), and not
    /// `ItemSlot::enchanted`, which is `has_foil()`. Three near-identical flags,
    /// one right answer; M93 recorded the same trap for the grindstone one
    /// field over.
    pub fn is_usable_for_crafting(&self, stack: ItemSlot) -> bool {
        // `isDamaged()` is `isDamageableItem() && getDamageValue() > 0` — so an
        // undamaged tool passes, and a stack that cannot take damage at all
        // passes whatever its `damage` says.
        let damaged = stack.max_damage.is_some() && stack.damage.unwrap_or(0) > 0;
        let text = self.text_of(stack);
        let enchanted = text.is_some_and(|t| t.is_enchanted);
        // `has(CUSTOM_NAME)` — **not** `getHoverName()`. This used to read the
        // merged `name` field, i.e. `custom_name OR item_name`, so a stack whose
        // patch set only `minecraft:item_name` was wrongly refused by the
        // solver. Splitting the field is what makes the right question askable.
        let named = text.is_some_and(|t| t.custom_name.is_some());
        !damaged && !enchanted && !named
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
    /// `LoomMenu`'s banner slot (M93g): `getItem() instanceof BannerItem`.
    LoomBanner,
    /// `LoomMenu`'s dye slot (M93g): `is(#LOOM_DYES) && has(DYE)`.
    LoomDye,
    /// `LoomMenu`'s pattern slot (M93g):
    /// `is(#LOOM_PATTERNS) && has(PROVIDES_BANNER_PATTERNS)`.
    ///
    /// Three kinds, because the loom tests three disjoint predicates in a
    /// fixed order and each slot enforces its own. One shared kind would let a
    /// dye into the pattern slot.
    LoomPattern,
    /// `CartographyTableMenu`'s MAP slot (M93f): `mayPlace` is
    /// `itemStack.has(DataComponents.MAP_ID)`.
    CartographyMap,
    /// `CartographyTableMenu`'s ADDITIONAL slot (M93f): `mayPlace` is
    /// `is(PAPER) || is(MAP) || is(GLASS_PANE)`.
    ///
    /// Two kinds rather than one, because the two slots take **disjoint** sets
    /// and the shift-click routes between them on the same two predicates. One
    /// shared kind would let a filled map into the paper slot.
    CartographyAdditional,
    /// `GrindstoneMenu`'s two repair slots (M93e): `mayPlace` is
    /// `itemStack.isDamageableItem() || EnchantmentHelper.hasAnyEnchantments(itemStack)`.
    ///
    /// The grindstone's `quickMoveStack` has **no** item predicate of its own —
    /// its guard is whether both repair slots are occupied — so this kind is
    /// the only thing that stops a stick being shift-clicked into one, via
    /// `moveItemStackTo`'s placement pass. That makes it load-bearing for the
    /// shift-click and not merely for an ordinary one.
    GrindstoneInput,
    /// `BeaconMenu`'s payment slot (M93): `mayPlace` is
    /// `itemStack.is(ItemTags.BEACON_PAYMENT_ITEMS)`.
    ///
    /// A kind of its own rather than `Plain`, because it is the one container
    /// slot in the transcribed set that refuses an ordinary item. Reporting it
    /// as plain would keep the *quick-move* exact — that guard checks the tag
    /// itself — but would let a **plain click** drop a stick into it locally,
    /// which the server rejects and pays for with a full state-id resync.
    BeaconPayment,
    /// `SmithingMenu`'s template slot 0 (M152): `templateItemTest::test`.
    SmithingTemplate,
    /// `SmithingMenu`'s base slot 1 (M152): `baseItemTest::test`.
    SmithingBase,
    /// `SmithingMenu`'s addition slot 2 (M152): `additionItemTest::test`.
    ///
    /// Three kinds for the same reason the loom has three
    /// ([`SlotKind::LoomBanner`]) — `createInputSlotDefinitions`
    /// (`SmithingMenu.java:53-63`) gives each slot its **own**
    /// `RecipePropertySet::test`, and the three sets are disjoint over vanilla
    /// data (measured on `data/minecraft/recipe/*.json`: base 37, template 19,
    /// addition 11, every pairwise intersection empty). One shared kind would
    /// let a netherite ingot into the template slot.
    ///
    /// Unlike every other kind here, these are **wire-derived**: the sets
    /// arrive on `update_recipes` rather than coming from the jar, so before
    /// that packet lands all three refuse everything. That is the safe
    /// direction and it is what vanilla does too —
    /// `ClientRecipeContainer.propertySet` is
    /// `getOrDefault(id, RecipePropertySet.EMPTY)`, and `EMPTY.test` is false
    /// for every item.
    SmithingAddition,
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
    /// `ItemTags.BEACON_PAYMENT_ITEMS` (M93) — the beacon's payment slot.
    ///
    /// A tag rather than a component, so it is read from the jar's own
    /// `data/minecraft/tags/item/beacon_payment_items.json` like
    /// `ItemTags.SPEARS` (M19) and the enchantment tags (M42), with the same
    /// caveat those carry: a **datapack** that retags it makes this wrong with
    /// no error anywhere.
    pub beacon_payment: bool,
    /// `stonecutterRecipes().acceptsInput` (M93b) — the stonecutter's input
    /// slot. Jar-derived from the stonecutting recipes, with M91's caveat: a
    /// datapack recipe change makes this wrong silently.
    ///
    /// **M152 correction.** This doc used to end "`update_recipes` is the
    /// authoritative source and Rewo does not decode it". Rewo decodes it now,
    /// and the packet carries the stonecutter's set as its second field — so
    /// the caveat is no longer *forced*, it is a **choice not yet revisited**.
    /// It is left jar-derived deliberately for one milestone: the wire set
    /// arrives only after `update_recipes`, so switching sources would make a
    /// stonecutter refuse everything for the first few hundred milliseconds of
    /// every session, and the honest fix is wire-when-present / jar-otherwise
    /// with its own witnesses. M152 uses the wire copy as an **oracle** for
    /// this table instead — see `containershot`.
    pub stonecuttable: bool,
    /// `templateItemTest.test` — `RecipePropertySet.SMITHING_TEMPLATE` (M152).
    ///
    /// **Wire-derived, unlike every other predicate on this struct.** The three
    /// smithing sets come from `update_recipes` rather than the jar, so all
    /// three are `false` until that packet arrives. Vanilla behaves the same
    /// way for the same reason: `ClientRecipeContainer.propertySet` is
    /// `getOrDefault(id, RecipePropertySet.EMPTY)` and `EMPTY` refuses
    /// everything.
    pub smithing_template: bool,
    /// `baseItemTest.test` — `RecipePropertySet.SMITHING_BASE` (M152).
    pub smithing_base: bool,
    /// `additionItemTest.test` — `RecipePropertySet.SMITHING_ADDITION` (M152).
    pub smithing_addition: bool,
    /// `getItem() instanceof BannerItem` (M93g), from `#minecraft:banners`.
    ///
    /// **Not** the `minecraft:banner_patterns` prototype component, which the
    /// SHIELD also carries — see `loom_table`'s module docs.
    pub loom_banner: bool,
    /// **Both** halves of `isDyeItem` that the item alone can answer:
    /// `is(#LOOM_DYES) && prototype_has(DYE)` (M93g). The patch's removal is
    /// on the stack.
    pub loom_dye: bool,
    /// The same for `isPatternItem`: `is(#LOOM_PATTERNS) &&
    /// prototype_has(PROVIDES_BANNER_PATTERNS)` (M93g).
    pub loom_pattern: bool,
    /// `CartographyTableMenu`'s ADDITIONAL slot (M93f) —
    /// `is(PAPER) || is(MAP) || is(GLASS_PANE)`.
    ///
    /// Item identity, so the item table answers it. Note `minecraft:map` here
    /// is the **empty** map: `filled_map` is a different item and is routed by
    /// [`ItemSlot::has_map_id`] to the other slot entirely, which is what makes
    /// map-cloning work.
    pub cartography_additional: bool,
    /// Whether the item's **prototype** carries `minecraft:max_damage` (M93e).
    /// From the per-item component table, which M56 already generates.
    pub proto_max_damage: bool,
    /// Whether the item's **prototype** carries `minecraft:damage` (M93e).
    ///
    /// Carried separately from [`Self::proto_max_damage`] even though the two
    /// co-occur on every one of 26.2's items, because `isDamageableItem` tests
    /// them as separate terms and collapsing them would quietly stop being
    /// exact the version they diverge.
    pub proto_damage: bool,
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
    /// `ItemStack.isDamageableItem()` —
    /// `has(MAX_DAMAGE) && !has(UNBREAKABLE) && has(DAMAGE)` (M93e).
    ///
    /// `has` on a `PatchedDataComponentMap` resolves in three steps: a removed
    /// component is absent whatever the prototype says, then a patched one is
    /// present, then the prototype answers. `UNBREAKABLE` needs no prototype
    /// term — no item carries it, verified against the component table — so
    /// the patch bit is the whole answer there.
    pub fn is_damageable_item(stack: ItemSlot, props: ItemProps) -> bool {
        if stack.damage_component_removed {
            return false;
        }
        let has_max = props.proto_max_damage || stack.max_damage.is_some();
        let has_damage = props.proto_damage || stack.damage.is_some();
        has_max && !stack.unbreakable && has_damage
    }

    /// `Slot.mayPlace` — whether this slot accepts this stack.
    ///
    /// Takes the **stack** and not only its item, because vanilla's signature
    /// is `mayPlace(ItemStack)` and two of the predicates transcribed here read
    /// the stack's components rather than its id: a grindstone accepts a
    /// *damaged or enchanted* tool, which is a property of the stack.
    fn may_place(kind: SlotKind, stack: ItemSlot, props: ItemProps) -> bool {
        match kind {
            // `ResultSlot.mayPlace` is `return false`.
            SlotKind::Result => false,
            // `ArmorSlot.mayPlace` is `owner.isEquippableInSlot(stack, slot)`,
            // and an item with no equippable component is main-hand-only — so
            // absence refuses, it does not default to allowed.
            SlotKind::Armor(piece) => props.equips == Some(piece),
            // M93 — `itemStack.is(ItemTags.BEACON_PAYMENT_ITEMS)`.
            SlotKind::BeaconPayment => props.beacon_payment,
            // M93e — a grindstone's two repair slots share one predicate:
            // `isDamageableItem() || EnchantmentHelper.hasAnyEnchantments()`.
            // The second disjunct is what lets an **enchanted book** in, which
            // is not damageable at all — and it reads `stored_enchantments`
            // too, which `isEnchanted()` does not.
            SlotKind::GrindstoneInput => {
                Self::is_damageable_item(stack, props) || stack.any_enchantments
            }
            // M93f — the cartography table's two slots, whose predicates the
            // shift-click branch tests as well. Both are needed: the branch
            // chooses WHICH slot to try, `mayPlace` confirms it will take it.
            // M93g — the loom. The banner test is item identity alone; the
            // other two are conjunctions whose component half can be removed
            // by the patch, which is what `*_removed` answers.
            SlotKind::LoomBanner => props.loom_banner,
            SlotKind::LoomDye => props.loom_dye && !stack.dye_removed,
            SlotKind::LoomPattern => {
                props.loom_pattern && !stack.provides_banner_patterns_removed
            }
            SlotKind::CartographyMap => stack.has_map_id,
            SlotKind::CartographyAdditional => props.cartography_additional,
            // M152 — the smithing table's three, each its own
            // `RecipePropertySet::test` from `createInputSlotDefinitions`.
            //
            // **These MUST be listed even though the quick-move already tests
            // them**, because the arm below is `_ => true` and a new
            // restrictive kind is silently granted permission by it. The
            // compiler cannot catch that: the catch-all exists for the genuinely
            // permissive kinds (`Craft`, `Main`, `Hotbar`, `Offhand`, `Plain`),
            // so every restrictive kind since has had to be added by hand. Left
            // out, the shift-click stays exact and a PLAIN click predicts a
            // placement the server rejects, costing a full state-id resync —
            // which is the failure `SlotKind::BeaconPayment`'s doc describes.
            SlotKind::SmithingTemplate => props.smithing_template,
            SlotKind::SmithingBase => props.smithing_base,
            SlotKind::SmithingAddition => props.smithing_addition,
            _ => true,
        }
    }

    /// `Slot.allowModification` — `mayPickup && mayPlace(getItem())`.
    ///
    /// Only the result slot ever answers false, and only because it refuses
    /// every placement including its own contents. It is what stops a partial
    /// take out of the crafting output.
    fn allow_modification(kind: SlotKind, stack: ItemSlot, occupant: ItemProps) -> bool {
        Self::may_place(kind, stack, occupant)
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
                if Self::may_place(kind, held, hp) {
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
                if Self::may_place(kind, held, hp) {
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
                    if amount > 0 && (!partial || Self::allow_modification(kind, stack, cp)) {
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
/// `ContainerInput.PICKUP` — the ordinary click, and the enum's zero.
pub const CONTAINER_INPUT_PICKUP: i32 = 0;
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
                if !Self::may_place(kind, *moving, p) {
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
    /// Where a shift-click from `slot` sends its stack: the destination
    /// ranges **to try in order**, each with whether it fills backwards.
    ///
    /// `quickMoveStack` is a per-menu-class **override** in vanilla, so this
    /// dispatches on the menu's shape rather than computing one (M90). An
    /// untranscribed menu returns `None`, which the caller turns into "not
    /// predictable — not sent": moving nothing is inert, where sending a
    /// shift-click under another menu's rules moves the wrong stack to the
    /// wrong place and the server applies it.
    ///
    /// # Why a chain and not one range (M92e)
    ///
    /// Most menus try exactly one destination, but `CraftingMenu` writes
    ///
    /// ```java
    /// if (!moveItemStackTo(stack, 1, 10, false)) {          // the grid
    ///     if (slotIndex < 37) moveItemStackTo(stack, 37, 46, false);
    ///     else                moveItemStackTo(stack, 10, 37, false);
    /// }
    /// ```
    ///
    /// and `moveItemStackTo` returns *whether anything moved*, so the second
    /// destination is reached only when the first took nothing. A single-range
    /// return cannot express that: it must either always try the grid (and
    /// never cross-move once the grid is full) or never try it. So the vector
    /// is the fallback chain, and the caller takes the first entry that moves
    /// something.
    ///
    /// Every other menu returns a one-element chain, which is the same
    /// behaviour it had before.
    fn quick_move_destination(
        &self,
        slot: usize,
        item: ItemSlot,
        slots: &[Option<ItemSlot>],
        p: ItemProps,
    ) -> Option<Vec<(std::ops::Range<usize>, bool)>> {
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
                return Some(vec![if slot < container_slots {
                    (container_slots..slots.len(), true)
                } else {
                    (0..container_slots, false)
                }]);
            }
            QuickMove::Furnace => {
                // AbstractFurnaceMenu.quickMoveStack. The literal ranges are
                // 3, 30 and 39 in vanilla; a furnace is 3 container slots plus
                // the player's 36, so they are container / container+27 / end.
                let (ingredient, fuel, result) = (0usize, 1usize, 2usize);
                let player = 3usize;
                let hotbar = player + 27;
                let end = slots.len();
                return Some(vec![if slot == result {
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
                }]);
            }
            // M92e — the crafting family, the two shapes that needed the chain.
            QuickMove::Crafting => {
                // CraftingMenu: result 0, grid 1..10, player 10..46.
                let (grid, player, hotbar, end) = (1usize, 10usize, 37usize, 46usize);
                return Some(match slot {
                    // The result fills the player BACKWARDS, so a craft lands
                    // in the hotbar's right-hand slots first.
                    0 => vec![(player..end, true)],
                    // A player slot tries the GRID first and only cross-moves
                    // when the grid took nothing. This is the branch that made
                    // the chain necessary, and it is why shift-clicking in a
                    // crafting table fills the grid rather than shuffling your
                    // inventory — unlike `InventoryMenu`, whose 2x2 grid is not
                    // a shift-click destination at all.
                    s if (player..end).contains(&s) => vec![
                        (grid..player, false),
                        if s < hotbar {
                            (hotbar..end, false)
                        } else {
                            (player..hotbar, false)
                        },
                    ],
                    // A grid slot goes back to the player, forwards.
                    _ => vec![(player..end, false)],
                });
            }
            // M93 — the single-input family. Three shapes, not one.
            QuickMove::Merchant => {
                // MerchantMenu: trades 0 and 1, result 2, player 3..39.
                //
                // The whole method, and it consults nothing: no predicate, no
                // recipe, no trade list. A player stack is never routed into
                // slots 0 or 1 — vanilla will not load a trade for you.
                let (player, hotbar, end) = (3usize, 30usize, 39usize);
                return Some(vec![match slot {
                    2 => (player..end, true),
                    0 | 1 => (player..end, false),
                    s if s < hotbar => (hotbar..end, false),
                    _ => (player..hotbar, false),
                }]);
            }
            QuickMove::ItemCombiner { result_slot } => {
                // ItemCombinerMenu, i.e. the anvil. The ranges are derived,
                // not literal: `getInventorySlotStart` is `resultSlot + 1`,
                // and the player's 27 + 9 follow it.
                let player = result_slot + 1;
                let hotbar = player + 27;
                let end = hotbar + 9;
                return Some(vec![if slot == result_slot {
                    (player..end, true)
                } else if slot < result_slot {
                    (player..end, false)
                } else {
                    // THE guard, and it is not a fallback chain (M92e's
                    // shape). `canMoveIntoInputSlots` is the inherited `true`
                    // for the anvil, so this arm always wins and the two
                    // main/hotbar arms below it in vanilla are unreachable:
                    // an anvil genuinely does not cross-move your inventory.
                    // If the input slots are full, `move_stack_to` reports
                    // nothing moved and the click sends nothing — which is
                    // vanilla's `return ItemStack.EMPTY`, not a bug.
                    (0..result_slot, false)
                }]);
            }
            QuickMove::Smithing => {
                // SmithingMenu (M152). Same class as the anvil, and NOT the
                // same routing — see `QuickMove::Smithing`'s docs. The ranges
                // are `ItemCombinerMenu`'s, derived from `getResultSlot() = 3`.
                let (result_slot, player) = (3usize, 4usize);
                let hotbar = player + 27;
                let end = hotbar + 9;

                if slot == result_slot {
                    return Some(vec![(player..end, true)]);
                }
                if slot < result_slot {
                    return Some(vec![(player..end, false)]);
                }

                // `canMoveIntoInputSlots` — each disjunct conjoined with its
                // own slot being EMPTY, so this reads occupancy and not just
                // the item. A second netherite ingot is refused while the
                // first still sits in the addition slot.
                let accepted = (p.smithing_template && slots[0].is_none())
                    || (p.smithing_base && slots[1].is_none())
                    || (p.smithing_addition && slots[2].is_none());
                if accepted {
                    return Some(vec![(0..result_slot, false)]);
                }

                // THE arms that make this menu different. For the anvil the
                // guard is the inherited `true`, so control never reaches
                // here and `QuickMove::ItemCombiner` rightly omits them. A
                // smithing table refusing your stack cross-moves it instead of
                // doing nothing.
                return Some(vec![if slot < hotbar {
                    (hotbar..end, false)
                } else {
                    (player..hotbar, false)
                }]);
            }
            QuickMove::Beacon => {
                // BeaconMenu: payment 0, player 1..37.
                let (player, hotbar, end) = (1usize, 28usize, 37usize);
                if slot == 0 {
                    return Some(vec![(player..end, true)]);
                }
                // Empty slot AND tagged AND a single item. The count test
                // lives in the branch rather than in `mayPlace`, so two
                // diamonds cross-move where one diamond is claimed.
                if slots[0].is_none() && p.beacon_payment && item.count == 1 {
                    return Some(vec![(0..1, false)]);
                }
                // Unlike the combiner's, this guard falling through reaches
                // the cross-move, because it is a sibling `else if` rather
                // than a condition on the destination.
                //
                // Vanilla has a fifth arm here — `moveItemStackTo(stack, 1,
                // 37, false)` — which is dead: arms 3 and 4 already cover
                // 1..37 and slot 0 is arm 1, so no index reaches it. Not
                // transcribed, because transcribing unreachable code invites
                // a later reader to "fix" the ranges that make it unreachable.
                return Some(vec![if slot < hotbar {
                    (hotbar..end, false)
                } else {
                    (player..hotbar, false)
                }]);
            }
            QuickMove::Loom => {
                // LoomMenu: banner 0, dye 1, pattern 2, result 3,
                // player 4..40.
                let (player, hotbar, end) = (4usize, 31usize, 40usize);
                // Each conjunction's component half, resolved here so the
                // three branch tests read as the three vanilla predicates.
                let is_dye = p.loom_dye && !item.dye_removed;
                let is_pattern = p.loom_pattern && !item.provides_banner_patterns_removed;
                return Some(vec![match slot {
                    3 => (player..end, true),
                    0 | 1 | 2 => (player..end, false),
                    // Tested in this order, and each branch CONSUMES — a
                    // banner with slot 0 already taken moves nothing rather
                    // than falling through to the dye slot or the hotbar.
                    _ if p.loom_banner => (0..1, false),
                    _ if is_dye => (1..2, false),
                    _ if is_pattern => (2..3, false),
                    s if s < hotbar => (hotbar..end, false),
                    _ => (player..hotbar, false),
                }]);
            }
            QuickMove::Cartography => {
                // CartographyTableMenu: map 0, additional 1, result 2,
                // player 3..39.
                let (player, hotbar, end) = (3usize, 30usize, 39usize);
                return Some(vec![match slot {
                    2 => (player..end, true),
                    0 | 1 => (player..end, false),
                    // `has(MAP_ID)` is tested FIRST, and it is what separates a
                    // FILLED map from an empty one: `filled_map` carries the
                    // component and goes to slot 0, while `minecraft:map` is a
                    // different item with no component and falls through to
                    // the additional slot below. That split is map-cloning.
                    _ if item.has_map_id => (0..1, false),
                    // Vanilla writes this arm as a triple negation —
                    // `!is(PAPER) && !is(MAP) && !is(GLASS_PANE)` -> cross-move
                    // — so the paper slot is the branch reached when the stack
                    // IS one of the three. Transcribing the negation as
                    // written and inverting the arms is the same thing said
                    // forwards, and much harder to misread.
                    _ if p.cartography_additional => (1..2, false),
                    s if s < hotbar => (hotbar..end, false),
                    _ => (player..hotbar, false),
                }]);
            }
            QuickMove::Grindstone => {
                // GrindstoneMenu: repair 0 and 1, result 2, player 3..39.
                let (player, hotbar, end) = (3usize, 30usize, 39usize);
                return Some(vec![match slot {
                    2 => (player..end, true),
                    0 | 1 => (player..end, false),
                    // The guard is about the SLOTS, not the item — the only
                    // menu here arranged that way. Both repair slots occupied
                    // means there is nowhere to put anything, so it degrades
                    // to the ordinary main/hotbar cross-move.
                    _ if slots[0].is_some() && slots[1].is_some() => {
                        if slot < hotbar {
                            (hotbar..end, false)
                        } else {
                            (player..hotbar, false)
                        }
                    }
                    // Otherwise it always TRIES the repair slots, whatever the
                    // item is — and `SlotKind::GrindstoneInput`'s `mayPlace` is
                    // what turns a stick away. When it does, the move reports
                    // nothing moved and vanilla `return`s, so a stick
                    // shift-clicked into an empty grindstone moves NOTHING
                    // rather than cross-moving. `p` is unused here for exactly
                    // that reason: the branch does not consult the item.
                    _ => {
                        let _ = p;
                        (0..2, false)
                    }
                }]);
            }
            QuickMove::Stonecutter => {
                // StonecutterMenu: input 0, result 1, player 2..38.
                let (player, hotbar, end) = (2usize, 29usize, 38usize);
                return Some(vec![match slot {
                    // The result fills the player backwards. NOTE the tail
                    // vanilla runs after this one and Rewo does not model:
                    // `if (slotIndex == 1) player.drop(stack, false)` — a
                    // remainder that did not fit is DROPPED ON THE GROUND, not
                    // left in the result slot. Rewo has no dropped-item
                    // prediction, so with a nearly-full inventory its local
                    // view keeps the remainder and the server's does not; the
                    // next `container_set_slot` corrects it. Recorded rather
                    // than approximated, because predicting an entity spawn is
                    // a bigger claim than predicting a slot.
                    1 => (player..end, true),
                    0 => (player..end, false),
                    // A player slot holding something the stonecutter cuts.
                    // Consumes: if slot 0 is occupied by something else this
                    // moves NOTHING rather than reaching the cross-move, which
                    // is the same `return ItemStack.EMPTY` shape the anvil has
                    // and the opposite of what the beacon does when ITS guard
                    // fails. Guard-fails-through and move-fails-out are two
                    // different exits and only the first cross-moves.
                    _ if p.stonecuttable => (0..1, false),
                    s if s < hotbar => (hotbar..end, false),
                    _ => (player..hotbar, false),
                }]);
            }
            QuickMove::Crafter { container_slots } => {
                // CrafterMenu: grid 0..9, player 9..45, result 45.
                //
                // Almost `SimpleContainer`, and the difference is the one
                // number that matters: its player range is `9..45`, NOT
                // `9..slots.size()`. Slot 45 is the `NonInteractiveResultSlot`
                // and vanilla excludes it from the destination outright rather
                // than relying on `mayPlace` to refuse it.
                let end = slots.len().saturating_sub(1);
                return Some(vec![if slot < container_slots {
                    (container_slots..end, true)
                } else {
                    (0..container_slots, false)
                }]);
            }
            QuickMove::PlayerInventory => {}
        }
        Some(vec![match slot {
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
                        return Some(vec![(armour..armour + 1, false)]);
                    }
                }
                let _ = item;
                match slot {
                    9..=35 => (36..45, false),
                    36..=44 => (9..36, false),
                    _ => (9..45, false),
                }
            }
        }])
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
            let chain = self.quick_move_destination(index, moving, &slots, p)?;
            // The first destination that moves something wins, which is what
            // vanilla's `if (!moveItemStackTo(a)) moveItemStackTo(b)` means
            // (M92e). A chain of one behaves exactly as the old single range.
            let mut moved = false;
            for (range, backwards) in chain {
                moved = self.move_stack_to(
                    &mut slots, &mut moving, range, backwards, props, &mut changed,
                )?;
                if moved {
                    break;
                }
            }
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

    /// `isUsableForCrafting` — three independent disqualifiers, and a plain
    /// stack passes all three.
    #[test]
    fn a_plain_stack_is_usable_for_crafting_and_three_things_disqualify_it() {
        let mut inv = Inventory::default();
        let plain = ItemSlot::plain(1, 1);
        assert!(inv.is_usable_for_crafting(plain));

        // 1. DAMAGED — and only if it is damageable at all.
        let chipped = ItemSlot { damage: Some(3), max_damage: Some(59), ..plain };
        assert!(!inv.is_usable_for_crafting(chipped));
        let fresh = ItemSlot { damage: Some(0), max_damage: Some(59), ..plain };
        assert!(inv.is_usable_for_crafting(fresh), "isDamaged needs damage > 0");
        // A stack that cannot take damage passes whatever its damage says —
        // `isDamageableItem()` gates the comparison.
        let odd = ItemSlot { damage: Some(3), max_damage: None, ..plain };
        assert!(inv.is_usable_for_crafting(odd));

        // 2. ENCHANTED, and 3. NAMED — both on the text side.
        let tagged = ItemSlot { components: 7, has_components: true, ..plain };
        inv.texts.insert(
            7,
            SlotText { is_enchanted: true, ..Default::default() },
        );
        assert!(!inv.is_usable_for_crafting(tagged));
        inv.texts.insert(
            7,
            SlotText {
                custom_name: Some(rewo_proto::nbt::Nbt::String("Bob".into())),
                ..Default::default()
            },
        );
        assert!(!inv.is_usable_for_crafting(tagged));
        // M161 — and the one this predicate used to get WRONG. The gate is
        // `has(CUSTOM_NAME)` (`Inventory.java:145-147`), not `getHoverName()`,
        // so a stack whose patch sets only `minecraft:item_name` is still
        // usable. It was refused while `SlotText` merged the two fields.
        inv.texts.insert(
            7,
            SlotText {
                item_name: Some(rewo_proto::nbt::Nbt::String("Blade".into())),
                ..Default::default()
            },
        );
        assert!(
            inv.is_usable_for_crafting(tagged),
            "item_name is not CUSTOM_NAME: the solver must still count this stack"
        );
        // A stack with a patch that carries neither still passes.
        inv.texts.insert(7, SlotText::default());
        assert!(inv.is_usable_for_crafting(tagged));
    }

    /// `isEnchanted()` is `ENCHANTMENTS` alone, which is `SlotText::is_enchanted`
    /// — **not** `any_enchantments` (enchantments OR stored) and not `enchanted`
    /// (`has_foil`). An enchanted BOOK is the case that separates them.
    #[test]
    fn the_crafting_gate_reads_isEnchanted_and_not_its_two_neighbours() {
        let mut inv = Inventory::default();
        let book = ItemSlot {
            components: 9,
            has_components: true,
            // Stored enchantments, so the wider predicate is true…
            any_enchantments: true,
            // …and the glint shows…
            enchanted: true,
            ..ItemSlot::plain(1, 1)
        };
        // …but `isEnchanted()` is false, so the book IS usable for crafting.
        inv.texts.insert(9, SlotText { is_enchanted: false, ..Default::default() });
        assert!(
            inv.is_usable_for_crafting(book),
            "a stored-enchantment book passes: isEnchanted reads ENCHANTMENTS alone"
        );
    }
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
            smithing_template: false,
            smithing_base: false,
            smithing_addition: false,
            max_stack: 64,
            equips: None,
            is_fuel: false,
            smeltable: [false; 3],
            beacon_payment: false,
            stonecuttable: false,
            cartography_additional: false,
            loom_banner: false,
            loom_dye: false,
            loom_pattern: false,
            proto_max_damage: false,
            proto_damage: false,
        })
    }

    /// An item that is BOTH fuel and smeltable, which is what a log is.
    fn log_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            smithing_template: false,
            smithing_base: false,
            smithing_addition: false,
            max_stack: 64,
            equips: None,
            is_fuel: true,
            smeltable: [false, true, false], // smeltable in a furnace only
            beacon_payment: false,
            stonecuttable: false,
            cartography_additional: false,
            loom_banner: false,
            loom_dye: false,
            loom_pattern: false,
            proto_max_damage: false,
            proto_damage: false,
        })
    }

    /// Fuel that is not smeltable, which is what coal is.
    fn coal_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            smithing_template: false,
            smithing_base: false,
            smithing_addition: false,
            max_stack: 64,
            equips: None,
            is_fuel: true,
            smeltable: [false; 3],
            beacon_payment: false,
            stonecuttable: false,
            cartography_additional: false,
            loom_banner: false,
            loom_dye: false,
            loom_pattern: false,
            proto_max_damage: false,
            proto_damage: false,
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

    // -- M92e: the crafting family, and the fallback chain ------------------

    /// A crafting table (menu 12): result 0, grid 1..=9, player 10..46.
    fn crafting_with(items: &[(usize, Option<ItemSlot>)]) -> Inventory {
        let mut c = Inventory::with_layout(crate::menu_layout::layout_of(12).unwrap());
        let mut v = vec![None; c.slot_count()];
        for &(slot, item) in items {
            v[slot] = item;
        }
        assert!(c.set_content(1, &v, None));
        c
    }

    #[test]
    fn a_crafting_tables_player_slot_fills_the_GRID_first() {
        // The behaviour the chain exists for, and the one that separates a
        // crafting table from `InventoryMenu`: vanilla tries `1..10` before it
        // cross-moves. A single-range implementation had to choose between
        // "always the grid" and "never the grid", and picking either loses
        // half the behaviour.
        let c = crafting_with(&[(20, stack(1, 5))]);
        let p = c.click_quick_move(20, &plain_props).expect("predictable");
        let into = moved_into(&p, 20);
        assert!(!into.is_empty());
        assert!(
            into.iter().all(|&s| (1..10).contains(&s)),
            "must land in the 3x3 grid, got {into:?}"
        );
    }

    #[test]
    fn a_full_grid_falls_back_to_the_cross_move() {
        // The other half of the chain: `moveItemStackTo` returns whether
        // anything moved, so the cross-move is reached only when the grid took
        // nothing. Fill the grid with a DIFFERENT item so nothing can merge.
        let mut items: Vec<(usize, Option<ItemSlot>)> =
            (1..10).map(|s| (s, stack(2, 64))).collect();
        items.push((20, stack(1, 5))); // a main-inventory slot
        let c = crafting_with(&items);
        let p = c.click_quick_move(20, &plain_props).expect("predictable");
        let into = moved_into(&p, 20);
        assert!(!into.is_empty(), "the fallback must actually run");
        assert!(
            into.iter().all(|&s| (37..46).contains(&s)),
            "a main slot with a full grid goes to the hotbar, got {into:?}"
        );
    }

    #[test]
    fn the_cross_move_direction_depends_on_which_half_the_source_is_in() {
        // Only reachable once the grid is full — which is why this witness
        // has to set the grid up too, and why an implementation that skipped
        // the grid would still pass a naive version of it.
        let grid: Vec<(usize, Option<ItemSlot>)> =
            (1..10).map(|s| (s, stack(2, 64))).collect();

        let mut from_hotbar = grid.clone();
        from_hotbar.push((40, stack(1, 5))); // a hotbar slot
        let c = crafting_with(&from_hotbar);
        let p = c.click_quick_move(40, &plain_props).expect("predictable");
        assert!(
            moved_into(&p, 40).iter().all(|&s| (10..37).contains(&s)),
            "a hotbar slot goes to the main inventory"
        );
    }

    #[test]
    fn the_crafting_result_fills_the_player_backwards() {
        // `moveItemStackTo(stack, 10, 46, true)` — a craft lands in the
        // hotbar's right-hand end first.
        let c = crafting_with(&[(0, stack(1, 5))]);
        let p = c.click_quick_move(0, &plain_props).expect("predictable");
        assert_eq!(
            moved_into(&p, 0).into_iter().max(),
            Some(45),
            "the last slot of the crafting menu"
        );
    }

    #[test]
    fn a_grid_slot_goes_back_to_the_player_forwards() {
        let c = crafting_with(&[(4, stack(1, 5))]);
        let p = c.click_quick_move(4, &plain_props).expect("predictable");
        let into = moved_into(&p, 4);
        assert!(into.iter().all(|&s| (10..46).contains(&s)), "{into:?}");
        assert_eq!(into.into_iter().min(), Some(10), "forwards, from the top");
    }

    #[test]
    fn the_two_crafting_menus_put_their_result_at_opposite_ends() {
        // `CraftingMenu` is result-first (slot 0) and `CrafterMenu` is
        // result-LAST (slot 45). A shared "the result is slot 0" constant
        // would make a crafter's output look like a plain slot and let a
        // click try to put something in it.
        let crafting = crate::menu_layout::layout_of(12).unwrap();
        let crafter = crate::menu_layout::layout_of(7).unwrap();
        assert_eq!(crafting.slot_kind(0), Some(SlotKind::Result));
        assert_eq!(crafting.slot_kind(45), Some(SlotKind::Plain));
        assert_eq!(crafter.slot_kind(0), Some(SlotKind::Plain));
        assert_eq!(crafter.slot_kind(45), Some(SlotKind::Result));
    }

    #[test]
    fn a_crafters_grid_slot_excludes_its_own_result_from_the_destination() {
        // `moveItemStackTo(stack, 9, 45, true)` — 45, not `slots.size()`.
        // Reversed, so without the exclusion the FIRST slot tried would be the
        // result slot itself.
        //
        // HONESTY NOTE, on M59's terms: **no single-point mutation breaks
        // this**, and it was mutation-tested to find that out. The result slot
        // is guarded twice — the range stops at 45, *and* `slot_kind` answers
        // `Result` there so `may_place` refuses it — so widening the range to
        // `9..46` alone changes nothing observable, and neither does dropping
        // the slot-kind alone. Removing BOTH is caught.
        //
        // That redundancy is **vanilla's own**, not an artefact of this port:
        // `CrafterMenu` passes 45 as the bound *and* `NonInteractiveResultSlot`
        // returns false from `mayPlace`. The transcription keeps the bound
        // because the source has it; the witness is kept as a statement of the
        // rule, because the realistic regression is someone "simplifying" the
        // crafter into `SimpleContainer { container_slots: 9 }`, whose
        // `slots.len()` bound would then be the only difference — and that
        // mutation IS caught, by this test and by the opposite-ends one.
        let mut c = Inventory::with_layout(crate::menu_layout::layout_of(7).unwrap());
        let mut v = vec![None; c.slot_count()];
        v[0] = stack(1, 5);
        assert!(c.set_content(1, &v, None));
        let p = c.click_quick_move(0, &plain_props).expect("predictable");
        let into = moved_into(&p, 0);
        assert!(!into.is_empty());
        assert!(
            into.iter().all(|&s| (9..45).contains(&s)),
            "45 is the NonInteractiveResultSlot and must not receive, got {into:?}"
        );
        assert_eq!(into.into_iter().max(), Some(44), "backwards, from 44");
    }

    #[test]
    fn a_crafters_player_slot_goes_to_its_nine_grid_slots() {
        let mut c = Inventory::with_layout(crate::menu_layout::layout_of(7).unwrap());
        let mut v = vec![None; c.slot_count()];
        v[20] = stack(1, 5);
        assert!(c.set_content(1, &v, None));
        let p = c.click_quick_move(20, &plain_props).expect("predictable");
        assert!(moved_into(&p, 20).iter().all(|&s| s < 9));
    }

    #[test]
    fn a_one_element_chain_behaves_exactly_as_the_single_range_did() {
        // The regression guard for the structural change: every shape that is
        // not the crafting table returns a chain of one, and a chain of one
        // must take the first destination and stop. If the loop ever ran on
        // past a successful move, a chest's shift-click would keep going into
        // a second range.
        let c = chest_with(0, stack(1, 5));
        let p = c.click_quick_move(0, &plain_props).unwrap();
        assert!(moved_into(&p, 0).iter().all(|&s| s >= 27));
        let f = furnace_with(20, stack(1, 5));
        let q = f.click_quick_move(20, &plain_props).expect("predictable");
        assert!(!moved_into(&q, 20).is_empty());
    }

    #[test]
    fn an_untranscribed_menu_declines_rather_than_borrowing_another_shape() {
        // A menu's quickMoveStack is its own; routing it as a chest would move
        // the wrong stack and the server would apply it. Declining sends
        // nothing.
        //
        // M90 wrote this against the **anvil**, which M93 then transcribed —
        // so the fixture named a real-but-uncovered menu and a later milestone
        // covered it. That is the rot M41 found in `swingshot` and M43 in two
        // `item_stack` fixtures, and there it was silent: the witness kept
        // passing while testing nothing. The remedy is the same one those took
        // — do not name an example that can be taken away. This asks the
        // registry which menus are still undone and proves the property on
        // every one of them, so the day the last is transcribed this fails
        // loudly rather than quietly asserting an empty loop.
        use crate::menu_layout::{QuickMove, REGISTRY};
        let undone: Vec<&'static crate::menu_layout::MenuLayout> = REGISTRY
            .iter()
            .filter(|m| m.quick_move() == QuickMove::Unimplemented)
            .collect();
        assert!(
            !undone.is_empty(),
            "every menu is transcribed — delete this test and the \
             `QuickMove::Unimplemented` arm with it, rather than leaving a \
             witness that iterates nothing"
        );
        for layout in undone {
            let mut menu = Inventory::with_layout(layout);
            let mut v = vec![None; menu.slot_count()];
            v[0] = stack(1, 5);
            menu.set_content(1, &v, None);
            assert!(
                menu.click_quick_move(0, &plain_props).is_none(),
                "{} predicted a shift-click with no transcribed routing",
                layout.name
            );
        }
    }

    // -- M93: the single-input family --------------------------------------

    /// Build one of the M93 menus with a single stack somewhere in it.
    fn single_input_menu(protocol_id: i32, slot: usize, item: Option<ItemSlot>) -> Inventory {
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(protocol_id).unwrap());
        let mut v = vec![None; m.slot_count()];
        v[slot] = item;
        assert!(m.set_content(1, &v, None));
        m
    }

    fn payment_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            beacon_payment: true,
            ..plain_props(0).unwrap()
        })
    }

    // ------------------------------------------------- M152: smithing table

    /// A smithing table (menu 21) with `item` at menu index `slot`.
    fn smithing_menu(slot: usize, item: Option<ItemSlot>) -> Inventory {
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(21).unwrap());
        let mut v = vec![None; m.slot_count()];
        v[slot] = item;
        assert!(m.set_content(1, &v, None));
        m
    }

    /// An item the smithing table accepts into the slot named by the flag.
    fn smith_props(template: bool, base: bool, addition: bool) -> ItemProps {
        ItemProps {
            smithing_template: template,
            smithing_base: base,
            smithing_addition: addition,
            ..plain_props(0).unwrap()
        }
    }

    /// **THE M152 finding.** Smithing is the only `ItemCombiner` whose
    /// cross-move arms are reachable, because it is the only one that overrides
    /// `canMoveIntoInputSlots` — so the guard can FAIL, and arms 4 and 5 run.
    ///
    /// The anvil's pair of tests above assert the opposite for the same class,
    /// which is what makes this a claim about the override and not about the
    /// ranges. A regression that routed smithing through
    /// `QuickMove::ItemCombiner { result_slot: 3 }` would move **nothing**
    /// here — correct-looking, and wrong only on this one menu.
    #[test]
    fn a_smithing_table_cross_moves_what_it_refuses() {
        // Main inventory (menu index 4 + 9 = 13), item accepted by nothing.
        let m = smithing_menu(13, stack(1, 5));
        let refused = |_id: i32| Some(smith_props(false, false, false));
        let p = m
            .click_quick_move(13, &refused)
            .expect("a refused stack must still cross-move, not vanish");
        let touched: Vec<u16> = p.changed.iter().map(|(i, _)| *i).collect();
        assert!(
            touched.iter().any(|&i| (31..40).contains(&i)),
            "main -> hotbar is arm 4 and it must run; touched {touched:?}"
        );
        assert!(
            !touched.iter().any(|&i| i < 4),
            "a refused stack must not reach an input slot; touched {touched:?}"
        );
    }

    /// The accepted case: the guard wins and consumes, exactly as the anvil's
    /// does. Paired with the test above, this is what proves the guard is
    /// *evaluated* rather than pinned to one answer — a routing hard-coded to
    /// either branch passes one of the two and fails the other.
    #[test]
    fn a_smithing_table_claims_what_it_accepts() {
        let m = smithing_menu(13, stack(1, 5));
        let accepted = |_id: i32| Some(smith_props(false, true, false));
        let p = m.click_quick_move(13, &accepted).expect("predictable");
        let touched: Vec<u16> = p.changed.iter().map(|(i, _)| *i).collect();
        assert!(
            touched.iter().any(|&i| i < 4),
            "an accepted stack must reach an input slot; touched {touched:?}"
        );
        assert!(
            !touched.iter().any(|&i| (31..40).contains(&i)),
            "the guard consumes, so the cross-move must NOT also run; touched {touched:?}"
        );
    }

    /// The guard reads the menu's OCCUPANCY, not just the item: each disjunct
    /// is conjoined with `!getSlot(n).hasItem()`.
    ///
    /// So the same item, with the same props, routes two different ways
    /// depending on whether its target slot is already taken — which a pure
    /// item predicate cannot express. This is the second netherite ingot.
    #[test]
    fn a_smithing_guard_is_refused_once_its_own_slot_is_full() {
        let accepted = |_id: i32| Some(smith_props(false, true, false));

        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(21).unwrap());
        let mut v = vec![None; m.slot_count()];
        // Slot 1 is the BASE slot, and it is taken by a different id so the
        // merge pass cannot top it up either.
        v[1] = stack(9, 1);
        v[13] = stack(1, 5);
        assert!(m.set_content(1, &v, None));

        let p = m
            .click_quick_move(13, &accepted)
            .expect("a refused stack still cross-moves");
        let touched: Vec<u16> = p.changed.iter().map(|(i, _)| *i).collect();
        assert!(
            touched.iter().any(|&i| (31..40).contains(&i)),
            "a full base slot must send this to the hotbar; touched {touched:?}"
        );
    }

    /// The result slot empties toward the player REVERSED, and the three input
    /// slots forward — `ItemCombinerMenu`'s arms 1 and 2, which smithing
    /// inherits unchanged. Reversed means it fills from the hotbar's
    /// right-hand end, because `addStandardInventorySlots` appends the hotbar
    /// last.
    #[test]
    fn a_smithing_result_empties_backwards_and_an_input_forwards() {
        let refused = |_id: i32| Some(smith_props(false, false, false));

        let from_result = smithing_menu(3, stack(1, 1));
        let r: Vec<u16> = from_result
            .click_quick_move(3, &refused)
            .expect("result is always predictable")
            .changed
            .iter()
            .map(|(i, _)| *i)
            .collect();
        assert!(
            r.iter().any(|&i| (31..40).contains(&i)),
            "the result slot fills the hotbar's far end first; touched {r:?}"
        );

        let from_input = smithing_menu(1, stack(1, 1));
        let i: Vec<u16> = from_input
            .click_quick_move(1, &refused)
            .expect("input is always predictable")
            .changed
            .iter()
            .map(|(i, _)| *i)
            .collect();
        assert!(
            i.iter().any(|&s| (4..31).contains(&s)),
            "an input slot empties into the main inventory first; touched {i:?}"
        );
    }

    /// The three slots enforce three DIFFERENT predicates, so a plain click is
    /// refused by the two slots the item does not belong to.
    ///
    /// This grades `may_place`, not the quick-move — and it is the half a
    /// catch-all `_ => true` silently breaks, since the compiler cannot see a
    /// missing arm. Without it the shift-click stays exact while an ordinary
    /// click predicts a placement the server rejects.
    #[test]
    fn the_three_smithing_slots_do_not_share_a_predicate() {
        use crate::menu_layout::layout_of;
        let l = layout_of(21).unwrap();
        assert_eq!(l.slot_kind(0), Some(SlotKind::SmithingTemplate));
        assert_eq!(l.slot_kind(1), Some(SlotKind::SmithingBase));
        assert_eq!(l.slot_kind(2), Some(SlotKind::SmithingAddition));
        assert_eq!(l.slot_kind(3), Some(SlotKind::Result));

        // A base item is accepted by slot 1 and refused by 0 and 2.
        let base_only = smith_props(false, true, false);
        let s = stack(1, 1).unwrap();
        assert!(!Inventory::may_place(SlotKind::SmithingTemplate, s, base_only));
        assert!(Inventory::may_place(SlotKind::SmithingBase, s, base_only));
        assert!(!Inventory::may_place(SlotKind::SmithingAddition, s, base_only));

        // And with no wire sets at all, every one refuses — which is vanilla
        // before `update_recipes`, via `getOrDefault(id, EMPTY)`.
        let none = smith_props(false, false, false);
        for k in [
            SlotKind::SmithingTemplate,
            SlotKind::SmithingBase,
            SlotKind::SmithingAddition,
        ] {
            assert!(!Inventory::may_place(k, s, none), "{k:?} accepted with no sets");
        }
    }

    #[test]
    fn an_anvils_input_branch_consumes_and_never_cross_moves() {
        // THE M93 finding. `canMoveIntoInputSlots` defaults to true, and the
        // branch it guards RETURNS rather than falling through, so vanilla's
        // two main/hotbar arms are unreachable for an anvil. A stack in the
        // player's main inventory therefore goes to an INPUT slot (0 or 1),
        // not to the hotbar the way it would in a chest.
        let anvil = single_input_menu(8, 10, stack(1, 5));
        let p = anvil.click_quick_move(10, &plain_props).expect("predictable");
        let touched: Vec<u16> = p.changed.iter().map(|(i, _)| *i).collect();
        assert!(
            touched.iter().any(|&i| i < 2),
            "an anvil must fill an input slot, touched {touched:?}"
        );
        assert!(
            !touched.iter().any(|&i| (30..39).contains(&i)),
            "an anvil must NOT cross-move into the hotbar, touched {touched:?}"
        );
    }

    #[test]
    fn an_anvil_with_full_inputs_moves_nothing_at_all() {
        // The other half of "consumes": once both inputs are taken there is no
        // second destination, so vanilla returns EMPTY and this client sends
        // nothing. Moving nothing is the correct answer, not a missing case.
        //
        // `None` on its own would be a weak witness — it is also what an
        // untranscribed menu answers, so a regression that reverted the anvil
        // to `Unimplemented` would pass this half. The pair is the witness:
        // the SAME anvil with one input free must answer `Some`, which only a
        // transcribed routing can do.
        let full = {
            let mut a = Inventory::with_layout(crate::menu_layout::layout_of(8).unwrap());
            let mut v = vec![None; a.slot_count()];
            // Two DIFFERENT ids, so the merge pass cannot top either up.
            v[0] = stack(7, 1);
            v[1] = stack(8, 1);
            v[10] = stack(1, 5);
            assert!(a.set_content(1, &v, None));
            a
        };
        assert!(
            full.click_quick_move(10, &plain_props).is_none(),
            "a full anvil has nowhere to put it and must send nothing"
        );

        let one_free = {
            let mut a = Inventory::with_layout(crate::menu_layout::layout_of(8).unwrap());
            let mut v = vec![None; a.slot_count()];
            v[0] = stack(7, 1);
            v[10] = stack(1, 5);
            assert!(a.set_content(1, &v, None));
            a
        };
        let p = one_free
            .click_quick_move(10, &plain_props)
            .expect("one free input is still predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 1),
            "it must land in the free input slot, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_merchants_player_slot_cross_moves_and_never_loads_a_trade() {
        // MerchantMenu consults nothing — and specifically never routes a
        // player stack into trade slots 0 or 1.
        let m = single_input_menu(19, 3, stack(1, 5));
        let p = m.click_quick_move(3, &plain_props).expect("predictable");
        let touched: Vec<u16> = p.changed.iter().map(|(i, _)| *i).collect();
        assert!(
            !touched.iter().any(|&i| i < 3),
            "a merchant must not fill its own trade slots, touched {touched:?}"
        );
        assert!(
            touched.iter().any(|&i| (30..39).contains(&i)),
            "a main-inventory stack must reach the hotbar, touched {touched:?}"
        );
    }

    #[test]
    fn a_beacon_claims_one_payment_item_but_not_a_pair() {
        // `stack.getCount() == 1` lives in the quickMoveStack branch, not in
        // mayPlace — so the SAME item routes two different ways by count.
        let one = single_input_menu(9, 1, stack(1, 1));
        let p = one.click_quick_move(1, &payment_props).expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "a single payment item belongs in the beacon slot, {:?}",
            p.changed
        );

        let two = single_input_menu(9, 1, stack(1, 2));
        let p = two.click_quick_move(1, &payment_props).expect("predictable");
        assert!(
            !p.changed.iter().any(|(i, _)| *i == 0),
            "a PAIR must cross-move like an ordinary item, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_beacons_untagged_item_falls_through_to_the_cross_move() {
        // Unlike the combiner's guard, the beacon's is a sibling `else if`, so
        // failing it reaches the main/hotbar arms rather than moving nothing.
        let b = single_input_menu(9, 1, stack(1, 1));
        let p = b.click_quick_move(1, &plain_props).expect("predictable");
        assert!(
            !p.changed.iter().any(|(i, _)| *i == 0),
            "an untagged item must not enter the payment slot, {:?}",
            p.changed
        );
        assert!(
            p.changed.iter().any(|(i, _)| (28..37).contains(i)),
            "and it must still reach the hotbar, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_beacon_with_an_occupied_payment_slot_cross_moves_instead() {
        let mut b = Inventory::with_layout(crate::menu_layout::layout_of(9).unwrap());
        let mut v = vec![None; b.slot_count()];
        v[0] = stack(7, 1);
        v[1] = stack(1, 1);
        assert!(b.set_content(1, &v, None));
        let p = b.click_quick_move(1, &payment_props).expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| (28..37).contains(i)),
            "an occupied payment slot must not swallow a second, {:?}",
            p.changed
        );
    }

    fn cuttable_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            stonecuttable: true,
            ..plain_props(0).unwrap()
        })
    }

    #[test]
    fn a_stonecuttable_stack_goes_to_the_input_slot_not_the_hotbar() {
        let s = single_input_menu(24, 2, stack(1, 5));
        let p = s.click_quick_move(2, &cuttable_props).expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "stone belongs in the stonecutter's input slot, {:?}",
            p.changed
        );
        assert!(
            !p.changed.iter().any(|(i, _)| (29..38).contains(i)),
            "and must not also reach the hotbar, {:?}",
            p.changed
        );
    }

    #[test]
    fn an_uncuttable_stack_falls_through_to_the_cross_move() {
        let s = single_input_menu(24, 2, stack(1, 5));
        let p = s.click_quick_move(2, &plain_props).expect("predictable");
        assert!(
            !p.changed.iter().any(|(i, _)| *i == 0),
            "a stick must not enter the input slot, {:?}",
            p.changed
        );
        assert!(
            p.changed.iter().any(|(i, _)| (29..38).contains(i)),
            "and it must reach the hotbar, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_blocked_input_slot_moves_nothing_rather_than_cross_moving() {
        // The distinction the three M93 menus turn on. When the stonecutter's
        // GUARD fails (an uncuttable item) vanilla falls through to the
        // cross-move — the test above. When the guard passes but the MOVE
        // fails, `moveItemStackTo` returns false and vanilla `return`s, so
        // nothing happens at all. Two different exits from one branch, and a
        // shape that tried the cross-move as a fallback would pass the other
        // two witnesses and fail this one.
        let mut s = Inventory::with_layout(crate::menu_layout::layout_of(24).unwrap());
        let mut v = vec![None; s.slot_count()];
        v[0] = stack(7, 1); // a DIFFERENT item, so no merge is possible
        v[2] = stack(1, 5);
        assert!(s.set_content(1, &v, None));
        assert!(
            s.click_quick_move(2, &cuttable_props).is_none(),
            "a blocked input slot must move nothing, not divert to the hotbar"
        );
        // Paired positive, so `None` is not confusable with "menu not
        // transcribed": clear the input and the same click must predict.
        let free = single_input_menu(24, 2, stack(1, 5));
        assert!(
            free.click_quick_move(2, &cuttable_props).is_some(),
            "with the input free the same click is predictable"
        );
    }

    #[test]
    fn the_stonecutters_input_slot_accepts_anything_an_ordinary_click_puts_there() {
        // Unlike the beacon's, this predicate is branch-only: slot 0 is a bare
        // `Slot` with no `mayPlace` override, so vanilla lets you drop a stick
        // in by hand. Giving it a SlotKind of its own would be stricter than
        // vanilla and would mispredict a placement the server accepts.
        let mut s = Inventory::with_layout(crate::menu_layout::layout_of(24).unwrap());
        assert!(s.set_content(1, &vec![None; s.slot_count()], None));
        s.set_carried(stack(1, 1));
        let p = s
            .click_pickup(0, 0, &plain_props)
            .expect("a pickup click is always sendable");
        assert_eq!(
            p.changed,
            vec![(0u16, stack(1, 1))],
            "an ordinary click may place an uncuttable item in the input slot"
        );
    }

    // -- M93g: the loom ----------------------------------------------------

    fn loom_props(banner: bool, dye: bool, pattern: bool) -> impl Fn(i32) -> Option<ItemProps> {
        move |_id| {
            Some(ItemProps {
                loom_banner: banner,
                loom_dye: dye,
                loom_pattern: pattern,
                ..plain_props(0).unwrap()
            })
        }
    }

    #[test]
    fn the_looms_three_predicates_route_to_three_different_slots() {
        for (i, props) in [
            loom_props(true, false, false),
            loom_props(false, true, false),
            loom_props(false, false, true),
        ]
        .into_iter()
        .enumerate()
        {
            let m = single_input_menu(18, 4, stack(1, 1));
            let p = m.click_quick_move(4, &props).expect("predictable");
            assert!(
                p.changed.iter().any(|(s, _)| *s as usize == i),
                "predicate {i} must fill slot {i}, {:?}",
                p.changed
            );
        }
    }

    #[test]
    fn the_looms_predicates_are_tested_in_vanillas_order() {
        // Banner, then dye, then pattern. Only a stack satisfying more than
        // one can show the order, and no vanilla item does — so construct the
        // overlap, exactly as M93f's cartography precedence witness does.
        let m = single_input_menu(18, 4, stack(1, 1));
        let p = m
            .click_quick_move(4, &loom_props(true, true, true))
            .expect("predictable");
        assert!(
            p.changed.iter().any(|(s, _)| *s == 0),
            "banner is tested first, {:?}",
            p.changed
        );
        let m = single_input_menu(18, 4, stack(1, 1));
        let p = m
            .click_quick_move(4, &loom_props(false, true, true))
            .expect("predictable");
        assert!(
            p.changed.iter().any(|(s, _)| *s == 1),
            "dye is tested before pattern, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_removed_component_breaks_the_conjunction_though_the_tag_still_matches() {
        // THE reason `loom_dye` is a conjunction and not a tag lookup. The tag
        // half is item identity and cannot be patched; the component half can
        // be REMOVED, and `has()` is false for a removed component even when
        // the prototype carries it. A tag-only test routes this to the dye
        // slot and the server rejects it.
        let stripped = stack(1, 1).map(|s| ItemSlot {
            dye_removed: true,
            ..s
        });
        let m = single_input_menu(18, 4, stripped);
        let p = m
            .click_quick_move(4, &loom_props(false, true, false))
            .expect("predictable");
        assert!(
            !p.changed.iter().any(|(s, _)| *s == 1),
            "a dye whose DYE component was removed must not take the dye slot, {:?}",
            p.changed
        );
        assert!(
            p.changed.iter().any(|(s, _)| (31..40).contains(s)),
            "it cross-moves instead, {:?}",
            p.changed
        );

        // ...and the two removals must not be interchangeable: stripping
        // PROVIDES_BANNER_PATTERNS from a dye changes nothing about it.
        let other = stack(1, 1).map(|s| ItemSlot {
            provides_banner_patterns_removed: true,
            ..s
        });
        let m = single_input_menu(18, 4, other);
        let p = m
            .click_quick_move(4, &loom_props(false, true, false))
            .expect("predictable");
        assert!(
            p.changed.iter().any(|(s, _)| *s == 1),
            "the OTHER component's removal is irrelevant to a dye, {:?}",
            p.changed
        );
    }

    #[test]
    fn the_pattern_conjunction_breaks_on_its_own_removal_too() {
        // Found by a surviving mutation. The dye witness above covers one half
        // of a SYMMETRIC pair, and testing one of two mirrored terms leaves
        // the other free to be deleted — `is_pattern`'s removal term was.
        let stripped = stack(1, 1).map(|s| ItemSlot {
            provides_banner_patterns_removed: true,
            ..s
        });
        let m = single_input_menu(18, 4, stripped);
        let p = m
            .click_quick_move(4, &loom_props(false, false, true))
            .expect("predictable");
        assert!(
            !p.changed.iter().any(|(s, _)| *s == 2),
            "a pattern item whose component was removed must not take the pattern slot, {:?}",
            p.changed
        );
        // ...and the mirror of the mirror: a removed DYE is irrelevant to it.
        let other = stack(1, 1).map(|s| ItemSlot {
            dye_removed: true,
            ..s
        });
        let m = single_input_menu(18, 4, other);
        let p = m
            .click_quick_move(4, &loom_props(false, false, true))
            .expect("predictable");
        assert!(
            p.changed.iter().any(|(s, _)| *s == 2),
            "the other component's removal is irrelevant to a pattern, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_plain_click_respects_the_conjunctions_removal_term_as_well() {
        // The second surviving mutation. Every plain-click witness above uses
        // a stack with no removals, so `may_place`'s removal term was never
        // exercised on that path — only on the quick-move's. The two are
        // separate code and both must carry it, or an ordinary click predicts
        // a placement the server rejects.
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(18).unwrap());
        assert!(m.set_content(1, &vec![None; m.slot_count()], None));
        let dye = loom_props(false, true, false);

        m.set_carried(stack(1, 1));
        assert!(
            !m.click_pickup(1, 0, &dye).unwrap().changed.is_empty(),
            "an intact dye is placeable by hand"
        );

        m.set_carried(stack(1, 1).map(|s| ItemSlot {
            dye_removed: true,
            ..s
        }));
        assert!(
            m.click_pickup(1, 0, &dye).unwrap().changed.is_empty(),
            "one whose DYE component was removed is not"
        );

        // The same for the pattern slot, so this witness is not itself half a
        // symmetric pair.
        let pat = loom_props(false, false, true);
        m.set_carried(stack(1, 1));
        assert!(!m.click_pickup(2, 0, &pat).unwrap().changed.is_empty());
        m.set_carried(stack(1, 1).map(|s| ItemSlot {
            provides_banner_patterns_removed: true,
            ..s
        }));
        assert!(m.click_pickup(2, 0, &pat).unwrap().changed.is_empty());
    }

    #[test]
    fn a_loom_input_branch_consumes_rather_than_trying_the_next_slot() {
        // All three branches return on failure. A banner with slot 0 taken
        // moves nothing — it does not fall through to the dye slot, nor to
        // the hotbar.
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(18).unwrap());
        let mut v = vec![None; m.slot_count()];
        v[0] = stack(7, 1);
        v[4] = stack(1, 1);
        assert!(m.set_content(1, &v, None));
        assert!(
            m.click_quick_move(4, &loom_props(true, false, false)).is_none(),
            "an occupied banner slot must move nothing"
        );
        let free = single_input_menu(18, 4, stack(1, 1));
        assert!(free
            .click_quick_move(4, &loom_props(true, false, false))
            .is_some());
    }

    #[test]
    fn the_looms_three_slots_refuse_each_others_items_on_a_plain_click() {
        // Each slot enforces its own predicate, so one shared SlotKind would
        // let a dye into the pattern slot.
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(18).unwrap());
        assert!(m.set_content(1, &vec![None; m.slot_count()], None));
        let kinds = [
            loom_props(true, false, false),
            loom_props(false, true, false),
            loom_props(false, false, true),
        ];
        for (owner, props) in kinds.iter().enumerate() {
            for slot in 0..3usize {
                m.set_carried(stack(1, 1));
                let changed = m.click_pickup(slot as i32, 0, props).unwrap().changed;
                assert_eq!(
                    !changed.is_empty(),
                    slot == owner,
                    "predicate {owner} into slot {slot}: {changed:?}"
                );
            }
        }
    }

    // -- M93f: the cartography table --------------------------------------

    /// `minecraft:paper` / `minecraft:map` / `minecraft:glass_pane`.
    fn additional_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            cartography_additional: true,
            ..plain_props(0).unwrap()
        })
    }

    fn filled_map(id: i32) -> Option<ItemSlot> {
        stack(id, 1).map(|s| ItemSlot {
            has_map_id: true,
            ..s
        })
    }

    #[test]
    fn a_filled_map_and_an_empty_one_go_to_different_slots() {
        // THE case. `has(MAP_ID)` is tested first, so a filled map takes the
        // map slot; `minecraft:map` is a different item carrying no such
        // component, falls past that test, and is caught by
        // `is(PAPER) || is(MAP) || is(GLASS_PANE)` into the additional slot.
        // Routing both to one slot is what would break map cloning.
        let m = single_input_menu(23, 3, filled_map(1));
        let p = m.click_quick_move(3, &plain_props).expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "a filled map belongs in the MAP slot, {:?}",
            p.changed
        );

        let m = single_input_menu(23, 3, stack(1, 1));
        let p = m
            .click_quick_move(3, &additional_props)
            .expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 1),
            "paper / an empty map / a glass pane belongs in the ADDITIONAL slot, {:?}",
            p.changed
        );
    }

    #[test]
    fn the_map_component_wins_over_the_item_identity() {
        // The two tests are checked in a fixed order, and only a stack that is
        // BOTH can show it. A filled map is not one of the three items, so
        // vanilla's ordering is invisible on real data — construct the overlap
        // and the precedence becomes observable.
        let both = stack(1, 1).map(|s| ItemSlot {
            has_map_id: true,
            ..s
        });
        let m = single_input_menu(23, 3, both);
        let p = m
            .click_quick_move(3, &additional_props)
            .expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "MAP_ID is tested first, so it takes the map slot, {:?}",
            p.changed
        );
    }

    #[test]
    fn anything_else_cross_moves() {
        let m = single_input_menu(23, 3, stack(1, 5));
        let p = m.click_quick_move(3, &plain_props).expect("predictable");
        assert!(
            !p.changed.iter().any(|(i, _)| *i < 3),
            "a stick must not enter either input slot, {:?}",
            p.changed
        );
        assert!(
            p.changed.iter().any(|(i, _)| (30..39).contains(i)),
            "and it must reach the hotbar, {:?}",
            p.changed
        );
    }

    #[test]
    fn an_occupied_input_slot_moves_nothing_rather_than_cross_moving() {
        // Both input branches CONSUME: `moveItemStackTo` returning false is
        // followed by `return ItemStack.EMPTY`, not by a fallback. So a second
        // filled map with the map slot already taken moves nothing at all.
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(23).unwrap());
        let mut v = vec![None; m.slot_count()];
        v[0] = filled_map(7);
        v[3] = filled_map(1);
        assert!(m.set_content(1, &v, None));
        assert!(
            m.click_quick_move(3, &plain_props).is_none(),
            "an occupied map slot must move nothing, not divert to the hotbar"
        );
        // Paired positive, so `None` is not confusable with an untranscribed
        // menu: clear the slot and the same click predicts.
        let free = single_input_menu(23, 3, filled_map(1));
        assert!(free.click_quick_move(3, &plain_props).is_some());
    }

    #[test]
    fn the_two_input_slots_refuse_each_others_items_on_a_plain_click() {
        // Unlike the stonecutter's, these predicates are on the SLOTS as well
        // as in the branch — so an ordinary click is bound by them too, and
        // one shared `SlotKind` would let a filled map into the paper slot.
        let mut m = Inventory::with_layout(crate::menu_layout::layout_of(23).unwrap());
        assert!(m.set_content(1, &vec![None; m.slot_count()], None));

        // A filled map: accepted by slot 0, refused by slot 1.
        m.set_carried(filled_map(1));
        assert_eq!(
            m.click_pickup(0, 0, &plain_props).unwrap().changed,
            vec![(0u16, filled_map(1))],
            "the map slot takes a filled map"
        );
        assert!(
            m.click_pickup(1, 0, &plain_props).unwrap().changed.is_empty(),
            "the additional slot must refuse it"
        );

        // Paper: the mirror image.
        m.set_carried(stack(2, 1));
        assert!(
            m.click_pickup(0, 0, &additional_props)
                .unwrap()
                .changed
                .is_empty(),
            "the map slot must refuse paper"
        );
        assert_eq!(
            m.click_pickup(1, 0, &additional_props).unwrap().changed,
            vec![(1u16, stack(2, 1))],
            "the additional slot takes paper"
        );
    }

    // -- M93e: the grindstone --------------------------------------------

    /// A damaged tool: the prototype carries both damage components.
    fn tool_props(_id: i32) -> Option<ItemProps> {
        Some(ItemProps {
            proto_max_damage: true,
            proto_damage: true,
            max_stack: 1,
            ..plain_props(0).unwrap()
        })
    }

    fn enchanted_book(id: i32) -> Option<ItemSlot> {
        stack(id, 1).map(|s| ItemSlot {
            any_enchantments: true,
            ..s
        })
    }

    #[test]
    fn a_grindstone_takes_a_tool_and_turns_a_stick_away() {
        // The predicate lives in `mayPlace`, so the ONLY way it shows is
        // through `moveItemStackTo`'s placement pass — and when it refuses,
        // vanilla returns rather than cross-moving. A stick shift-clicked into
        // an empty grindstone therefore moves nothing at all.
        let g = single_input_menu(15, 3, stack(1, 1));
        let p = g.click_quick_move(3, &tool_props).expect("a tool is repairable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "a damageable tool belongs in a repair slot, {:?}",
            p.changed
        );

        let g = single_input_menu(15, 3, stack(1, 1));
        assert!(
            g.click_quick_move(3, &plain_props).is_none(),
            "a stick must move NOTHING — not to the repair slot, not to the hotbar"
        );
    }

    #[test]
    fn an_enchanted_book_is_accepted_though_it_is_not_damageable() {
        // The second disjunct, and the case `ItemSlot::enchanted` would miss
        // twice over: a book is not damageable, and its enchantments live in
        // `stored_enchantments`, which `isEnchanted()` does not read.
        let g = single_input_menu(15, 3, enchanted_book(1));
        let p = g
            .click_quick_move(3, &plain_props)
            .expect("an enchanted book is grindable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "an enchanted book belongs in a repair slot, {:?}",
            p.changed
        );
    }

    #[test]
    fn an_unbreakable_tool_is_not_damageable_and_is_turned_away() {
        // `isDamageableItem` is `has(MAX_DAMAGE) && !has(UNBREAKABLE) &&
        // has(DAMAGE)`. Dropping the middle term silently accepts every
        // unbreakable tool, which looks right until you try to grind one.
        let unbreakable = stack(1, 1).map(|s| ItemSlot {
            unbreakable: true,
            ..s
        });
        let g = single_input_menu(15, 3, unbreakable);
        assert!(
            g.click_quick_move(3, &tool_props).is_none(),
            "an unbreakable tool is not damageable"
        );
    }

    #[test]
    fn a_patch_that_removes_a_damage_component_makes_a_tool_undamageable() {
        // `has()` on a PatchedDataComponentMap is false for a REMOVED
        // component even when the prototype carries it. Modelling `has` as
        // "prototype or patch-set" alone would accept this.
        let stripped = stack(1, 1).map(|s| ItemSlot {
            damage_component_removed: true,
            ..s
        });
        let g = single_input_menu(15, 3, stripped);
        assert!(
            g.click_quick_move(3, &tool_props).is_none(),
            "a tool whose damage component was removed is not damageable"
        );
    }

    #[test]
    fn max_damage_alone_does_not_make_an_item_damageable() {
        // Found by a surviving mutation: dropping the `has(DAMAGE)` term
        // changed nothing, because every fixture set `proto_max_damage` and
        // `proto_damage` together — which is true of all 1537 vanilla items
        // and NOT true of a patched stack. A server that puts
        // `minecraft:max_damage` on a stick without `minecraft:damage` gets
        // `has(MAX_DAMAGE) && !has(UNBREAKABLE) && !has(DAMAGE)` = false, and
        // a two-term formula would hand it to the grindstone.
        //
        // This is why the two prototype flags are carried separately rather
        // than as one `damageable` bool.
        let patched = stack(1, 1).map(|s| ItemSlot {
            max_damage: Some(100),
            damage: None,
            ..s
        });
        let g = single_input_menu(15, 3, patched);
        assert!(
            g.click_quick_move(3, &plain_props).is_none(),
            "max_damage without damage is not damageable"
        );
        // The mirror: with `damage` patched in as well, it IS.
        let both = stack(1, 1).map(|s| ItemSlot {
            max_damage: Some(100),
            damage: Some(0),
            ..s
        });
        let g = single_input_menu(15, 3, both);
        let p = g
            .click_quick_move(3, &plain_props)
            .expect("both components present makes it damageable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 0),
            "and then the grindstone takes it, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_full_grindstone_cross_moves_because_its_guard_is_about_the_slots() {
        // The inversion worth having a witness for: every other menu here
        // guards on the ITEM and lets the slot accept anything. This one
        // guards on both repair slots being occupied, and then degrades to the
        // ordinary main/hotbar cross-move — for a TOOL, which the repair slots
        // would otherwise have taken.
        let mut g = Inventory::with_layout(crate::menu_layout::layout_of(15).unwrap());
        let mut v = vec![None; g.slot_count()];
        v[0] = stack(7, 1);
        v[1] = stack(8, 1);
        v[3] = stack(1, 1);
        assert!(g.set_content(1, &v, None));
        let p = g.click_quick_move(3, &tool_props).expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| (30..39).contains(i)),
            "with both repair slots full a tool cross-moves to the hotbar, {:?}",
            p.changed
        );
    }

    #[test]
    fn a_half_full_grindstone_still_takes_the_second_slot() {
        // The guard is `!input.isEmpty() && !additional.isEmpty()` — BOTH.
        // Reading it as "either" sends a tool to the hotbar while a repair
        // slot sits empty, which looks like a plausible menu and is wrong.
        let mut g = Inventory::with_layout(crate::menu_layout::layout_of(15).unwrap());
        let mut v = vec![None; g.slot_count()];
        v[0] = stack(7, 1);
        v[3] = stack(1, 1);
        assert!(g.set_content(1, &v, None));
        let p = g.click_quick_move(3, &tool_props).expect("predictable");
        assert!(
            p.changed.iter().any(|(i, _)| *i == 1),
            "the free repair slot must still take it, {:?}",
            p.changed
        );
    }

    #[test]
    fn every_single_input_menu_empties_its_output_slot_backwards() {
        // Found by a surviving mutation: flipping the stonecutter's result arm
        // from `true` to `false` changed nothing any witness could see, and
        // the same hole covered the anvil, beacon and merchant. `backwards`
        // is the difference between a taken result landing in the hotbar's
        // RIGHT-hand end — where `addStandardInventorySlots` appends it — and
        // in the first free main-inventory slot. Both look like "it moved".
        //
        // (menu id, the slot whose contents leave via the player range)
        for (id, source) in [(8i32, 2usize), (9, 0), (19, 2), (24, 1), (15, 2), (23, 2), (18, 3)] {
            let m = single_input_menu(id, source, stack(1, 1));
            let p = m
                .click_quick_move(source as i32, &plain_props)
                .unwrap_or_else(|| panic!("menu {id} declined"));
            let landed: Vec<u16> = p
                .changed
                .iter()
                .filter(|(i, v)| *i as usize != source && v.is_some())
                .map(|(i, _)| *i)
                .collect();
            let last = m.slot_count() - 1;
            assert_eq!(
                landed,
                vec![last as u16],
                "menu {id}: a backwards fill lands in the LAST player slot ({last}), \
                 forwards would land in the first"
            );
        }
    }

    #[test]
    fn every_transcribed_result_slot_refuses_a_placement() {
        // The three M93 menus each have a result slot whose `mayPlace` is
        // `false`, and reporting it as Plain would let a click drop something
        // into it that the server then rejects.
        use crate::menu_layout::layout_of;
        for (id, result) in [
            (8i32, 2usize),
            (9, usize::MAX),
            (19, 2),
            (24, 1),
            (15, 2),
            (23, 2),
            (18, 3),
        ] {
            let layout = layout_of(id).unwrap();
            for slot in 0..layout.slot_count() {
                let kind = layout.slot_kind(slot).expect("transcribed");
                let is_result = kind == SlotKind::Result;
                assert_eq!(
                    is_result,
                    slot == result,
                    "{} slot {slot} reported {kind:?}",
                    layout.name
                );
            }
        }
        // ...and the beacon's payment slot is its own kind, because the tag is
        // what makes it refuse a stick.
        assert_eq!(
            layout_of(9).unwrap().slot_kind(0),
            Some(SlotKind::BeaconPayment)
        );
    }

    #[test]
    fn a_plain_click_respects_the_beacons_tag_too_not_just_the_shift_click() {
        // The quick-move guard reads `beacon_payment` itself, so every witness
        // above passes with `may_place` reporting the slot as accept-anything.
        // This is the one that does not: a carried stack dropped straight into
        // the payment slot goes through `may_place`, and reporting the slot as
        // `Plain` there would predict a placement the server rejects — paid
        // for with a full state-id resync, which is the whole thing the click
        // prediction exists to avoid.
        let mut b = Inventory::with_layout(crate::menu_layout::layout_of(9).unwrap());
        assert!(b.set_content(1, &vec![None; b.slot_count()], None));

        b.set_carried(stack(1, 1));

        // The refusal shows as an empty change set, NOT as `None`. The two
        // click paths differ here and both match vanilla: `doClick`'s PICKUP
        // arm always sends its packet, so a refused placement is a click that
        // changed nothing, whereas a quick-move that moved nothing is the one
        // case `click_quick_move` declines outright. Asserting `is_none()`
        // here — which the first draft of this witness did — measures the
        // wrong path and fails against correct code.
        let refused = b
            .click_pickup(0, 0, &plain_props)
            .expect("a pickup click is always sendable");
        assert!(
            refused.changed.is_empty(),
            "an untagged item must not be predicted into the payment slot, {:?}",
            refused.changed
        );
        assert_eq!(
            refused.carried,
            stack(1, 1),
            "and it must stay on the cursor"
        );

        // ...and the same click with a tagged item must land, or the refusal
        // above would be indistinguishable from a dead code path.
        let placed = b
            .click_pickup(0, 0, &payment_props)
            .expect("a pickup click is always sendable");
        assert_eq!(
            placed.changed,
            vec![(0u16, stack(1, 1))],
            "a tagged item must still be placeable"
        );
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
            any_enchantments: false,
            unbreakable: false,
            damage_component_removed: false,
            has_map_id: false,
            dye_removed: false,
            provides_banner_patterns_removed: false,
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

/// The menu slot a SWAP button names in the **player's own** inventory (M93i).
///
/// `ContainerInput.SWAP`'s `buttonNum` is an *inventory index* — M69's third
/// coordinate system — and both `click_swap` and `CrafterScreen`'s
/// `player.getInventory().getItem(buttonNum)` resolve it the same way. An
/// out-of-range button is `None`, matching `Inventory.getItem`, which answers
/// EMPTY rather than throwing.
pub fn swap_button_menu_slot(button: i32) -> Option<usize> {
    if button == SWAP_OFFHAND_BUTTON {
        Some(OFFHAND_MENU_SLOT)
    } else if (0..HOTBAR_SIZE as i32).contains(&button) {
        Some(HOTBAR_MENU_START + button as usize)
    } else {
        None
    }
}

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
                if !Self::may_place(kind, s, p) {
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
                if !Self::may_place(kind, s, p) {
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
        if amount < stack.count && !Self::allow_modification(kind, stack, p) {
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
            if Self::allow_modification(kind, stack, props(stack.item_id)?) {
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
                if !Self::allow_modification(self.layout.slot_kind(i)?, target, tp) {
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
        if !Self::may_place(kind, carried, p) {
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

#[cfg(test)]
mod m93i_swap_button {
    use super::*;

    /// M93i — the SWAP button's inventory index, shared by `click_swap` and
    /// the crafter's toggle gate.
    #[test]
    fn a_swap_button_names_the_hotbar_or_the_offhand_and_nothing_else() {
        assert_eq!(swap_button_menu_slot(0), Some(HOTBAR_MENU_START));
        assert_eq!(swap_button_menu_slot(8), Some(HOTBAR_MENU_START + 8));
        assert_eq!(swap_button_menu_slot(SWAP_OFFHAND_BUTTON), Some(OFFHAND_MENU_SLOT));
        // 9..40 is not a swap target — the range is the hotbar plus the one
        // literal 40, not a contiguous span, which M40 records as the reason
        // the check REJECTS rather than clamping.
        for b in [9, 10, 39, 41, -1] {
            assert_eq!(swap_button_menu_slot(b), None, "button {b}");
        }
    }
}
