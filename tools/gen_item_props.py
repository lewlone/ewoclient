"""Machine-extract the per-item properties the container needs into a Rust
table (`crates/rewo-data/src/item_props_table.rs`): `minecraft:max_stack_size`,
`minecraft:max_damage`, `minecraft:equippable` (slot + asset) and
`minecraft:rarity`.

Why this exists
---------------
None of these values is on the wire. `ItemStack.OPTIONAL_STREAM_CODEC` sends
`count + item-registry-id + DataComponentPatch`, and the patch carries only
*deltas from the item's prototype*; `DataComponents.COMMON_ITEM_COMPONENTS`
sets `MAX_STACK_SIZE` and `RARITY` on every item and the equippable items set
`EQUIPPABLE` in their own properties. So the client resolves them from the
**item id**, which makes the mapping vanilla data, and this is where it comes
from.

Both feed `AbstractContainerMenu`'s PICKUP branch:

- `Slot.getMaxStackSize(stack)` is `min(slot cap, stack.getMaxStackSize())`,
  and every arm caps on it — `safeInsert`'s `transferableItemCount`, the
  same-item merge's headroom, and the swap's
  `carried.getCount() <= slot.getMaxStackSize(carried)` guard.
- `ArmorSlot.mayPlace` is `owner.isEquippableInSlot(stack, slot)`, which reads
  `EQUIPPABLE.slot()` and compares. An item with no equippable component is
  placeable only in the main hand, so every armour slot refuses it.

Getting the first two wrong is not cosmetic. The client sends the server its
*prediction* of every changed slot; a wrong cap or a wrongly-allowed placement
predicts a wrong slot, the server's `HashedStack.matches` fails, and the whole
container is resynchronised — which looks like clicks bouncing back.

`minecraft:rarity` is the one that is *only* visible: it colours a stack's
hover name, and reading it from the patch alone (which is what a client that
never consults the prototype does) paints every one of 26.2's 115 non-common
items white. A music disc's name is yellow in vanilla.

Ground truth is the datagen item-component report, never community docs
(REWO_PLAN §11):

    <APPDATA>/EwoClient/rewo/<version>/datagen/generated/reports/
        minecraft/components/item/<item>.json

plus three decompiled classes, read to pin the meaning of the values:

    net/minecraft/world/item/Item.java              (the stack-size defaults)
    net/minecraft/world/entity/EquipmentSlot.java   (the slot names)
    net/minecraft/world/item/Rarity.java            (the rarity names + ids)

Re-run after a version bump:

    python tools/gen_item_props.py

Fails loud rather than defaulting: a missing report tree, an item whose report
carries no `max_stack_size`, a non-integer or out-of-range size, an equippable
slot name the decompiled enum does not declare, a rarity name it does not
declare, or defaults that no longer match the decompile are all hard errors.
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
DAMAGE_KEY = "minecraft:max_damage"
EQUIP_KEY = "minecraft:equippable"
RARITY_KEY = "minecraft:rarity"
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


def rarities():
    """`Rarity`'s `(serialized name -> id)`, and the default `getRarity` takes.

    The **id** is what the table stores, because that is also what the wire
    carries: `Rarity.STREAM_CODEC` is `ByteBufCodecs.idMapper(BY_ID, r -> r.id)`,
    so a `minecraft:rarity` patch entry and a row here are the same number in
    the same space. Storing the name would need a second mapping at every
    comparison.

    The default is read out of `ItemStack.getRarity` rather than assumed to be
    COMMON — the table below is a delta from it, so a version that changed it
    would silently mark 1,422 items non-default.
    """
    src = read(os.path.join(DECOMP, "world", "item", "Rarity.java"))
    found = re.findall(r'^\s*([A-Z_]+)\((\d+),\s*"([a-z_]+)"', src, re.M)
    if not found:
        die("could not parse `NAME(id, \"name\", ...)` constants out of Rarity "
            "— the enum's shape changed and the ids this table emits are "
            "unverified")
    by_name = {}
    by_const = {}
    for const, ident, name in found:
        by_name[name] = int(ident)
        by_const[const] = int(ident)

    stack = read(os.path.join(DECOMP, "world", "item", "ItemStack.java"))
    m = re.search(r"getOrDefault\(DataComponents\.RARITY,\s*Rarity\.([A-Z_]+)\)",
                  stack)
    if not m:
        die("could not find `getOrDefault(DataComponents.RARITY, Rarity.X)` in "
            "ItemStack — the default this table is a delta from is no longer "
            "parseable")
    if m.group(1) not in by_const:
        die(f"ItemStack defaults rarity to Rarity.{m.group(1)}, which Rarity "
            f"does not declare")
    return by_name, by_const[m.group(1)]


def main():
    if not os.path.isdir(REPORT):
        die(f"missing item-component report at {REPORT} — run the datagen "
            f"first (REWO_PLAN §11)")
    default, absolute_max = stack_size_defaults()
    equipment_slots()
    rarity_ids, default_rarity = rarities()

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
        asset = None
        equippable = components.get(EQUIP_KEY)
        if equippable is not None:
            if not isinstance(equippable, dict):
                die(f"{name}: `{EQUIP_KEY}` is {equippable!r}, not an object")
            # `Equippable.assetId()` — which `assets/minecraft/equipment/
            # <asset>.json` describes this piece's armour layers. Optional:
            # a carved pumpkin is worn and has no armour model.
            asset = equippable.get("asset_id")
            if asset is not None and not isinstance(asset, str):
                die(f"{name}: `{EQUIP_KEY}` has a non-string `asset_id`")
            slot = equippable.get("slot")
            if not isinstance(slot, str):
                die(f"{name}: `{EQUIP_KEY}` has no string `slot`")
            if slot not in SLOT_VARIANTS:
                die(f"{name}: equippable slot {slot!r} is not one this table "
                    f"emits ({SLOT_VARIANTS})")

        # `minecraft:max_damage` — the denominator of a durability bar.
        # Only damageable items carry it, so its absence is the answer for
        # everything else rather than a gap.
        max_damage = components.get(DAMAGE_KEY)
        if max_damage is not None:
            if not isinstance(max_damage, int) or isinstance(max_damage, bool):
                die(f"{name}: `{DAMAGE_KEY}` is {max_damage!r}, not an integer")
            if max_damage <= 0:
                die(f"{name}: `{DAMAGE_KEY}` is {max_damage}, which cannot be a "
                    f"denominator")

        # `minecraft:rarity` — the hover name's colour. Every item carries one
        # through COMMON_ITEM_COMPONENTS, so a missing key is a report-shape
        # change rather than "common".
        if RARITY_KEY not in components:
            die(f"{name}: no `{RARITY_KEY}` — every item is expected to carry "
                f"one through COMMON_ITEM_COMPONENTS")
        rarity_name = components[RARITY_KEY]
        if rarity_name not in rarity_ids:
            die(f"{name}: rarity {rarity_name!r} is not one `Rarity` declares "
                f"({sorted(rarity_ids)})")
        rarity = rarity_ids[rarity_name]
        rarity = rarity if rarity != default_rarity else None

        if (size != default or slot is not None or max_damage is not None
                or rarity is not None):
            rows[item] = (size if size != default else None, slot, max_damage,
                          asset, rarity)

    body = "\n".join(
        '    ("minecraft:{}", {}, {}, {}, {}, {}),'.format(
            item,
            "None" if s is None else f"Some({s})",
            "None" if q is None else f"Some(EquipSlot::{q.capitalize()})",
            "None" if d is None else f"Some({d})",
            "None" if a is None else f'Some("{a}")',
            "None" if r is None else f"Some({r})")
        for item, (s, q, d, a, r) in sorted(rows.items()))

    assets = len({a for _, _, _, a, _ in rows.values() if a is not None})
    sizes, slots, rare, damaged = {}, {}, {}, 0
    for s, q, d, _, r in rows.values():
        if s is not None:
            sizes[s] = sizes.get(s, 0) + 1
        if q is not None:
            slots[q] = slots.get(q, 0) + 1
        if r is not None:
            rare[r] = rare.get(r, 0) + 1
        if d is not None:
            damaged += 1
    n_sized = sum(sizes.values())
    size_summary = ", ".join(f"{n} at {v}" for v, n in sorted(sizes.items()))
    slot_summary = ", ".join(f"{n} {v}" for v, n in sorted(slots.items()))
    id_to_name = {v: k for k, v in rarity_ids.items()}
    rarity_summary = ", ".join(f"{n} {id_to_name[v]}"
                               for v, n in sorted(rare.items()))
    variants = "\n".join(f"    {n.capitalize()}," for n in SLOT_VARIANTS)
    default_rarity_name = id_to_name[default_rarity]

    text = f'''//! Per-item stack size, equippable slot, durability and rarity — GENERATED,
//! do not edit.
//!
//! Regenerate with `python tools/gen_item_props.py` after a version bump.
//!
//! Source: the datagen per-item component report for {VERSION}. Of {total}
//! items, {n_sized} differ from `Item.DEFAULT_MAX_STACK_SIZE` = {default}
//! ({size_summary}), {sum(slots.values())} carry `minecraft:equippable`
//! ({slot_summary}), {damaged} carry `minecraft:max_damage`, {assets}
//! distinct equipment assets are named, and {sum(rare.values())} differ from
//! `Rarity.{default_rarity_name.upper()}` ({rarity_summary}). Items with none
//! of these are not listed.
//!
//! The first two feed the container click arithmetic. A wrong cap or a
//! wrongly-allowed armour placement predicts a wrong slot, the server's
//! `HashedStack.matches` fails, and the container resynchronises — which looks
//! like a click that bounced. The rarity is purely visible, and is here for
//! the same reason: it lives in the prototype, so a client that reads only the
//! component patch paints all {sum(rare.values())} of these white.

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

/// `Rarity.{default_rarity_name.upper()}`'s id — the value
/// `ItemStack.getRarity`'s `getOrDefault(DataComponents.RARITY, ...)` falls
/// back to, and the one the table below is a delta from.
pub const DEFAULT_RARITY: i32 = {default_rarity};

/// Every item that differs from the defaults in at least one column, sorted by
/// name so a binary search finds it. A `None` in any column means the default.
pub const ITEM_PROPS: &[(
    &str,
    Option<i32>,
    Option<EquipSlot>,
    Option<i32>,
    Option<&str>,
    Option<i32>,
)] = &[
{body}
];

type Props = (
    Option<i32>,
    Option<EquipSlot>,
    Option<i32>,
    Option<&'static str>,
    Option<i32>,
);

fn lookup(name: &str) -> Option<Props> {{
    ITEM_PROPS
        .binary_search_by(|(n, _, _, _, _, _)| (*n).cmp(name))
        .ok()
        .map(|i| {{
            (
                ITEM_PROPS[i].1,
                ITEM_PROPS[i].2,
                ITEM_PROPS[i].3,
                ITEM_PROPS[i].4,
                ITEM_PROPS[i].5,
            )
        }})
}}

/// `Equippable.assetId()` — which `assets/minecraft/equipment/<asset>.json`
/// describes this item's armour layers (M46).
///
/// `None` covers two cases the caller must not confuse: an item that is not
/// equippable at all, and one that is worn but names no armour model (a carved
/// pumpkin). Both render no armour, which is why they share a return.
pub fn equip_asset(name: &str) -> Option<&'static str> {{
    lookup(name).and_then(|(_, _, _, a, _)| a)
}}

/// `getOrDefault(DataComponents.RARITY, Rarity.{default_rarity_name.upper()})`
/// — the **prototype** half of `ItemStack.getRarity`.
///
/// This is the one the wire cannot supply. A `minecraft:rarity` entry in the
/// patch overrides it (a plugin may), so the caller takes the patch first;
/// with no patch entry — which is the ordinary case for every stack in the
/// game — this is the answer, and defaulting to
/// `{default_rarity_name.upper()}` instead paints all {sum(rare.values())}
/// non-common items white.
///
/// The **enchantment promotion** is not here: it depends on the stack, not the
/// item. See `ItemStack.getRarity`'s switch.
pub fn rarity(name: &str) -> i32 {{
    lookup(name)
        .and_then(|(_, _, _, _, r)| r)
        .unwrap_or(DEFAULT_RARITY)
}}

/// `stack.getMaxDamage()` — the denominator of a durability bar, or `None` for
/// an item that cannot be damaged.
///
/// The **numerator** is `minecraft:damage`, which does travel on the wire as a
/// patch; this does not, because a pickaxe's 1561 is the same on every
/// pickaxe. A patch that overrides `max_damage` wins over this, which is why
/// the caller takes the patch's value first.
pub fn max_damage(name: &str) -> Option<i32> {{
    lookup(name).and_then(|(_, _, d, _, _)| d)
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
    lookup(name).and_then(|(s, _, _, _, _)| s).unwrap_or(DEFAULT_MAX_STACK)
}}

/// The slot an item can be equipped into, or `None` if it carries no
/// `minecraft:equippable`.
///
/// `LivingEntity.isEquippableInSlot` treats that absence as **main hand only**,
/// so an item without the component is refused by every armour slot — which is
/// why this returns an `Option` rather than defaulting to anything.
pub fn equip_slot(name: &str) -> Option<EquipSlot> {{
    lookup(name).and_then(|(_, q, _, _, _)| q)
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

    /// One item per rarity bucket, pinned by hand from the report. A music
    /// disc is the case that made this column necessary: the wire says
    /// nothing about it, so a client reading only the patch draws its name
    /// white where vanilla draws it yellow.
    #[test]
    fn the_rarity_buckets_are_present() {{
        assert_eq!(rarity("minecraft:dirt"), DEFAULT_RARITY);
        assert_eq!(rarity("minecraft:music_disc_13"), 1);
        assert_eq!(rarity("minecraft:enchanted_golden_apple"), 2);
        assert_eq!(rarity("minecraft:elytra"), 3);
    }}

    /// A durability bar needs both halves, and only one of them is on the
    /// wire.
    #[test]
    fn damageable_items_carry_a_maximum() {{
        assert_eq!(max_damage("minecraft:diamond_pickaxe"), Some(1561));
        assert_eq!(max_damage("minecraft:dirt"), None);
        assert_eq!(
            equip_asset("minecraft:diamond_chestplate"),
            Some("minecraft:diamond")
        );
        assert_eq!(equip_asset("minecraft:dirt"), None);
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
          f"({size_summary}; {slot_summary}; {rarity_summary}) "
          f"-> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
