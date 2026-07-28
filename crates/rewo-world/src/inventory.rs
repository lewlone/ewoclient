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

#[derive(Clone, Debug)]
pub struct Inventory {
    slots: [Option<ItemSlot>; MENU_SLOTS],
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
        Self {
            slots: [None; MENU_SLOTS],
            carried: None,
            selected: 0,
            state_id: 0,
            content_updates: 0,
            texts: std::collections::HashMap::new(),
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
        self.content_updates += 1;
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

    /// See [`Self::content_updates`] — a delta across a click of more than
    /// zero means the server rejected the prediction.
    pub fn content_updates(&self) -> u32 {
        self.content_updates
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
pub fn slot_position(slot: usize) -> Option<(i32, i32)> {
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

/// `AbstractContainerScreen.isHovering` — **an 18x18 box, not 16x16**.
///
/// The slot's icon is 16 px, but the test is `x >= left - 1 && x < left + w + 1`
/// with `w = 16`, so it reaches one pixel out on every side and the slots tile
/// without gaps. Using the icon's own rect instead leaves a one-pixel dead
/// cross between every pair of neighbours.
pub fn slot_contains(slot: usize, gui_x: f64, gui_y: f64) -> bool {
    let Some((left, top)) = slot_position(slot) else {
        return false;
    };
    let (left, top) = (left as f64, top as f64);
    gui_x >= left - 1.0 && gui_x < left + 17.0 && gui_y >= top - 1.0 && gui_y < top + 17.0
}

/// The menu slot under a GUI-relative point, or `None`.
///
/// The boxes overlap by their one-pixel bleed, and vanilla's `getHoveredSlot`
/// returns the **first** match in menu order, so this iterates rather than
/// computing an index.
pub fn slot_at(gui_x: f64, gui_y: f64) -> Option<usize> {
    (0..MENU_SLOTS).find(|&s| slot_contains(s, gui_x, gui_y))
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
        let kind = slot_kind(index)?;
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
        slots: &mut [Option<ItemSlot>; MENU_SLOTS],
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
                let kind = slot_kind(i)?;
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
                let kind = slot_kind(i)?;
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
    fn quick_move_destination(
        slot: usize,
        item: ItemSlot,
        slots: &[Option<ItemSlot>; MENU_SLOTS],
        p: ItemProps,
    ) -> Option<(std::ops::Range<usize>, bool)> {
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
        let kind = slot_kind(index)?;
        let source = self.slots[index]?;
        let p = props(source.item_id)?;
        let _ = kind;

        let mut slots = self.slots;
        let mut changed: Vec<SlotChange> = Vec::new();
        // Bounded rather than `loop`: each pass either empties the source or
        // fills one destination, and 46 slots cannot absorb more than that.
        for _ in 0..MENU_SLOTS {
            let Some(current) = slots[index] else { break };
            let mut moving = current;
            let (range, backwards) = Self::quick_move_destination(index, moving, &slots, p)?;
            let moved = Self::move_stack_to(
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
        let kind = slot_kind(target_index)?;
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
        let kind = slot_kind(index)?;
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
        let kind = slot_kind(index)?;
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
        let mut slots = self.slots;
        let mut changed: Vec<SlotChange> = Vec::new();
        let order: Vec<usize> = if button == 0 {
            (0..MENU_SLOTS).collect()
        } else {
            (0..MENU_SLOTS).rev().collect()
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
                if !Self::allow_modification(slot_kind(i)?, tp) {
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
        let Some(kind) = slot_kind(index) else {
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
            let kind_of = slot_kind(i)?;
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
