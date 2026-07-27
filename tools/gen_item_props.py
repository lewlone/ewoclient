"""Machine-extract the two per-item properties the container click arithmetic
needs into a Rust table (`crates/rewo-data/src/item_props_table.rs`):
`minecraft:max_stack_size` and `minecraft:equippable`'s slot.

Why this exists
---------------
Neither value is on the wire. `ItemStack.OPTIONAL_STREAM_CODEC` sends
`count + item-registry-id + DataComponentPatch`, and the patch carries only
*deltas from the item's prototype*; `DataComponents.COMMON_ITEM_COMPONENTS`
sets `MAX_STACK_SIZE` on every item and the equippable items set `EQUIPPABLE`
in their own properties. So the client resolves both from the **item id**,
which makes the mapping vanilla data, and this is where it comes from.

Both feed `AbstractContainerMenu`'s PICKUP branch:

- `Slot.getMaxStackSize(stack)` is `min(slot cap, stack.getMaxStackSize())`,
  and every arm caps on it — `safeInsert`'s `transferableItemCount`, the
  same-item merge's headroom, and the swap's
  `carried.getCount() <= slot.getMaxStackSize(carried)` guard.
- `ArmorSlot.mayPlace` is `owner.isEquippableInSlot(stack, slot)`, which reads
  `EQUIPPABLE.slot()` and compares. An item with no equippable component is
  placeable only in the main hand, so every armour slot refuses it.

Getting either wrong is not cosmetic. The client sends the server its
*prediction* of every changed slot; a wrong cap or a wrongly-allowed placement
predicts a wrong slot, the server's `HashedStack.matches` fails, and the whole
container is resynchronised — which looks like clicks bouncing back.

Ground truth is the datagen item-component report, never community docs
(REWO_PLAN §11):

    <APPDATA>/EwoClient/rewo/<version>/datagen/generated/reports/
        minecraft/components/item/<item>.json

plus two decompiled classes, read to pin the meaning of the values:

    net/minecraft/world/item/Item.java              (the stack-size defaults)
    net/minecraft/world/entity/EquipmentSlot.java   (the slot names)

Re-run after a version bump:

    python tools/gen_item_props.py

Fails loud rather than defaulting: a missing report tree, an item whose report
carries no `max_stack_size`, a non-integer or out-of-range size, an equippable
slot name the decompiled enum does not declare, or defaults that no longer
match the decompile are all hard errors.
"""
import json
import os
import re
import sys

VERSION = "26.2"
ROOT = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", VERSION)
REPORT = os.path.join(ROOT, "datagen", "generated", "reports",
                      "minecraft", "components", "item")
DECOMP = os.path.join(ROOT, "decompiled", "net", "minecraft")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "item_props_table.rs")

SIZE_KEY = "minecraft:max_stack_size"
EQUIP_KEY = "minecraft:equippable"
# The members `ArmorSlot`/`InventoryMenu` name, plus the two hands. A report
# naming anything else is a hard error rather than a dropped row.
SLOT_VARIANTS = ["head", "chest", "legs", "feet", "mainhand", "offhand",
                 "body", "saddle"]


def die(msg):
    print(f"gen_item_props: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        die(f"cannot read {path}: {e}")
        raise


def stack_size_defaults():
    """`Item.DEFAULT_MAX_STACK_SIZE` and `ABSOLUTE_MAX_STACK_SIZE`.

    The table stores only items that differ from the first, so if vanilla ever
    changed it the table would silently gain 1,200 wrong entries.
    """
    src = read(os.path.join(DECOMP, "world", "item", "Item.java"))
    m = re.search(r"\bDEFAULT_MAX_STACK_SIZE\s*=\s*(\d+)\s*;", src)
    if not m:
        die("could not find `DEFAULT_MAX_STACK_SIZE = N;` in Item — the "
            "default this table is a delta from is no longer parseable")
    a = re.search(r"\bABSOLUTE_MAX_STACK_SIZE\s*=\s*(\d+)\s*;", src)
    if not a:
        die("could not find `ABSOLUTE_MAX_STACK_SIZE = N;` in Item")
    return int(m.group(1)), int(a.group(1))


def equipment_slots():
    """`EquipmentSlot`'s serialized names, read rather than assumed."""
    src = read(os.path.join(DECOMP, "world", "entity", "EquipmentSlot.java"))
    names = set(re.findall(r'"([a-z_]+)"', src))
    missing = [n for n in SLOT_VARIANTS if n not in names]
    if missing:
        die(f"EquipmentSlot no longer declares {missing} — the slot names this "
            f"table emits are stale")
    return names


def main():
    if not os.path.isdir(REPORT):
        die(f"missing item-component report at {REPORT} — run the datagen "
            f"first (REWO_PLAN §11)")
    default, absolute_max = stack_size_defaults()
    equipment_slots()

    files = sorted(f for f in os.listdir(REPORT) if f.endswith(".json"))
    if not files:
        die(f"no item reports under {REPORT}")

    rows = {}
    total = 0
    for name in files:
        item = name[: -len(".json")]
        with open(os.path.join(REPORT, name), "r", encoding="utf-8") as f:
            doc = json.load(f)
        components = doc.get("components")
        if not isinstance(components, dict):
            die(f"{name}: no `components` object")

        if SIZE_KEY not in components:
            die(f"{name}: no `{SIZE_KEY}` — every item is expected to carry "
                f"one through COMMON_ITEM_COMPONENTS; a missing one means the "
                f"report shape changed")
        size = components[SIZE_KEY]
        if not isinstance(size, int) or isinstance(size, bool):
            die(f"{name}: `{SIZE_KEY}` is {size!r}, not an integer")
        if not 1 <= size <= absolute_max:
            die(f"{name}: `{SIZE_KEY}` is {size}, outside 1..={absolute_max}")
        total += 1

        slot = None
        equippable = components.get(EQUIP_KEY)
        if equippable is not None:
            if not isinstance(equippable, dict):
                die(f"{name}: `{EQUIP_KEY}` is {equippable!r}, not an object")
            slot = equippable.get("slot")
            if not isinstance(slot, str):
                die(f"{name}: `{EQUIP_KEY}` has no string `slot`")
            if slot not in SLOT_VARIANTS:
                die(f"{name}: equippable slot {slot!r} is not one this table "
                    f"emits ({SLOT_VARIANTS})")

        if size != default or slot is not None:
            rows[item] = (size if size != default else None, slot)

    body = "\n".join(
        '    ("minecraft:{}", {}, {}),'.format(
            item,
            "None" if s is None else f"Some({s})",
            "None" if q is None else f"Some(EquipSlot::{q.capitalize()})")
        for item, (s, q) in sorted(rows.items()))

    sizes, slots = {}, {}
    for s, q in rows.values():
        if s is not None:
            sizes[s] = sizes.get(s, 0) + 1
        if q is not None:
            slots[q] = slots.get(q, 0) + 1
    n_sized = sum(sizes.values())
    size_summary = ", ".join(f"{n} at {v}" for v, n in sorted(sizes.items()))
    slot_summary = ", ".join(f"{n} {v}" for v, n in sorted(slots.items()))
    variants = "\n".join(f"    {n.capitalize()}," for n in SLOT_VARIANTS)

    text = f'''//! Per-item stack size and equippable slot — GENERATED, do not edit.
//!
//! Regenerate with `python tools/gen_item_props.py` after a version bump.
//!
//! Source: the datagen per-item component report for {VERSION}. Of {total}
//! items, {n_sized} differ from `Item.DEFAULT_MAX_STACK_SIZE` = {default}
//! ({size_summary}) and {sum(slots.values())} carry `minecraft:equippable`
//! ({slot_summary}). Items that are neither are not listed.
//!
//! Both feed the container click arithmetic. A wrong cap or a wrongly-allowed
//! armour placement predicts a wrong slot, the server's `HashedStack.matches`
//! fails, and the container resynchronises — which looks like a click that
//! bounced.

/// `EquipmentSlot`, restricted to the members an item can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquipSlot {{
{variants}
}}

/// `Item.DEFAULT_MAX_STACK_SIZE`, read from the decompile rather than assumed,
/// because the table below is a delta from it.
pub const DEFAULT_MAX_STACK: i32 = {default};

/// `Item.ABSOLUTE_MAX_STACK_SIZE` — the ceiling `stacksTo` asserts against.
pub const ABSOLUTE_MAX_STACK: i32 = {absolute_max};

/// Every item that is not (default stack size, not equippable), sorted by name
/// so a binary search finds it. A `None` size means the default.
pub const ITEM_PROPS: &[(&str, Option<i32>, Option<EquipSlot>)] = &[
{body}
];

fn lookup(name: &str) -> Option<(Option<i32>, Option<EquipSlot>)> {{
    ITEM_PROPS
        .binary_search_by(|(n, _, _)| (*n).cmp(name))
        .ok()
        .map(|i| (ITEM_PROPS[i].1, ITEM_PROPS[i].2))
}}

/// `stack.getMaxStackSize()` for an item name.
///
/// An item absent from the table takes [`DEFAULT_MAX_STACK`], which is correct
/// **only for a name that came out of the item registry** — the table lists
/// every real item that differs, so absence means "the default", not
/// "unknown". A name this build has never heard of is a different question,
/// and the caller must have failed to resolve it long before reaching here:
/// `Items::name` returns `None`, and the click path declines to predict rather
/// than guessing a cap.
pub fn max_stack_size(name: &str) -> i32 {{
    lookup(name).and_then(|(s, _)| s).unwrap_or(DEFAULT_MAX_STACK)
}}

/// The slot an item can be equipped into, or `None` if it carries no
/// `minecraft:equippable`.
///
/// `LivingEntity.isEquippableInSlot` treats that absence as **main hand only**,
/// so an item without the component is refused by every armour slot — which is
/// why this returns an `Option` rather than defaulting to anything.
pub fn equip_slot(name: &str) -> Option<EquipSlot> {{
    lookup(name).and_then(|(_, q)| q)
}}

#[cfg(test)]
mod tests {{
    use super::*;

    /// The binary search is only correct if the generator emitted the table
    /// sorted.
    #[test]
    fn the_table_is_sorted() {{
        assert!(ITEM_PROPS.windows(2).all(|w| w[0].0 < w[1].0));
    }}

    /// One item per stack-size bucket, pinned by hand from the report, so a
    /// regenerated table that collapsed to one number fails here.
    #[test]
    fn the_three_stack_buckets_are_present() {{
        assert_eq!(max_stack_size("minecraft:dirt"), 64);
        assert_eq!(max_stack_size("minecraft:ender_pearl"), 16);
        assert_eq!(max_stack_size("minecraft:diamond_sword"), 1);
    }}

    /// A registry item outside the table takes the default.
    #[test]
    fn an_item_outside_the_table_takes_the_default() {{
        assert_eq!(max_stack_size("minecraft:stone"), DEFAULT_MAX_STACK);
    }}

    /// The armour rule's inputs: a helmet names `head`, a shield names
    /// `offhand`, and dirt names nothing at all — which is what makes every
    /// armour slot refuse it.
    #[test]
    fn equippable_slots_are_read() {{
        assert_eq!(equip_slot("minecraft:diamond_helmet"), Some(EquipSlot::Head));
        assert_eq!(equip_slot("minecraft:shield"), Some(EquipSlot::Offhand));
        assert_eq!(equip_slot("minecraft:dirt"), None);
    }}

    /// A carved pumpkin is equippable **and** stacks to the default — it is in
    /// the table for the second reason only, which is the row a size-only
    /// table would have dropped.
    #[test]
    fn an_equippable_item_at_the_default_size_is_still_listed() {{
        assert_eq!(max_stack_size("minecraft:carved_pumpkin"), DEFAULT_MAX_STACK);
        assert_eq!(equip_slot("minecraft:carved_pumpkin"), Some(EquipSlot::Head));
    }}
}}
'''
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"gen_item_props: {total} items -> {len(rows)} rows "
          f"({size_summary}; {slot_summary}) -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
