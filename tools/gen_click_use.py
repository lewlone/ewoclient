#!/usr/bin/env python3
"""Extract which block class and which item class own a right-click, from the
decompiled 26.2 jar, into `crates/rewo-data/src/click_use_table.rs` (M229).

Why this exists
---------------
`Minecraft.startUseItem` sends `use_item_on` for a block under the crosshair
and then, only when the LOCAL result of `MultiPlayerGameMode.useItemOn` is a
PASS, goes on to `use_item` (eat, draw the bow, throw the pearl, fill the
bucket). The local result is vanilla's own client-side run of

    block.useItemOn(stack, ...)            -- TRY_WITH_EMPTY_HAND by default
      -> block.useWithoutItem(...)         -- PASS by default
    stack.useOn(context)                   -- PASS by default (Item.useOn)

so the client has to know, per block and per item, which class's override
answers. That is class-hierarchy data, and this script is where it comes from
(REWO_PLAN section 11: the decompile, never a wiki). The per-class RULES live in
`rewo-world/src/click_use.rs`; this table only says which class applies.

Emitted:

* `BLOCK_CLICK_CLASS` -- block registry name -> the most-derived class in its
  chain that overrides `useWithoutItem` or `useItemOn` (BlockBehaviour, the
  declaring base, excluded). Absent = both defaults: the click passes.
* `ITEM_CLICK_CLASS` -- item registry name -> the most-derived class in its
  chain that overrides `useOn` (Item, the declaring base, excluded), for the
  single-item declarations in `Items.java`. Block items are `BlockItem` or the
  subclass that overrides it; the colour / weathering collections are all
  block items and are covered by the caller's "the item is named after a
  block" rule instead.
* `BLOCK_USE_CLASSES` / `ITEM_USE_ON_CLASSES` -- every class that overrides,
  so a new override in a later version fails the Rust side's coverage test.

Run after a version bump:
    python tools/gen_click_use.py
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_block_light as gbl  # noqa: E402
import gen_block_props as gbp  # noqa: E402

DECOMP = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", "26.2",
                      "decompiled", "net", "minecraft")
ITEM_DIR = os.path.join(DECOMP, "world", "item")
OUT = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                   "crates", "rewo-data", "src", "click_use_table.rs")


def die(msg):
    print(f"gen_click_use: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    return open(path, encoding="utf8", errors="replace").read()


def overriding(dirpath, pattern):
    """Class name -> True for every class under `dirpath` whose own source
    declares a method matching `pattern`."""
    out = set()
    for dp, _dirs, files in os.walk(dirpath):
        for fn in files:
            if fn.endswith(".java") and re.search(pattern, read(os.path.join(dp, fn))):
                out.add(fn[: -len(".java")])
    return out


def item_superclasses():
    out = {}
    for dp, _dirs, files in os.walk(ITEM_DIR):
        for fn in files:
            if not fn.endswith(".java"):
                continue
            m = re.search(r"class\s+(\w+)(?:<[^>]*>)?\s+extends\s+(\w+)", read(os.path.join(dp, fn)))
            if m:
                out[m.group(1)] = m.group(2)
    return out


def most_derived(chain, overriders, base):
    for c in chain:
        if c == base:
            return None
        if c in overriders:
            return c
    return None


def blocks(method):
    """Block name -> the most-derived class overriding `method`
    (`useItemOn` or `useWithoutItem`), and the override set."""
    use_pat = r"protected\s+InteractionResult\s+" + method + r"\s*\("
    over = overriding(gbp.BLOCK_DIR, use_pat) - {"BlockBehaviour"}
    sups = gbp.superclasses()
    src = read(gbl.BLOCKS)
    id_tables = gbl.scan_id_tables()
    single_ids = gbl.scan_single_ids()
    known = set(json.load(open(gbl.REGISTRY, encoding="utf8")).keys())
    table = {}

    def record(name, cls):
        if f"minecraft:{name}" not in known or not cls:
            return
        c = most_derived(gbp.chain(sups, cls), over, "BlockBehaviour")
        if c:
            table[f"minecraft:{name}"] = c

    for m in re.finditer(r"public\s+static\s+final\s+Block\s+([A-Z0-9_]+)\s*=\s*\w+\s*\(", src, re.M):
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"Block(?:Item)?Ids\.(\w+)", body)
        name = single_ids.get(ref.group(1)) if ref else None
        record(name or m.group(1).lower(), gbl.impl_class(body))
    for m in re.finditer(
        r"public\s+static\s+final\s+(?:Color|WeatheringCopper)Collection<Block>\s+"
        r"[A-Z0-9_]+\s*=\s*\w+\s*\.\s*registerBlocks\s*\(", src, re.M
    ):
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"BlockItemIds\.(\w+)", body)
        cls = gbl.impl_class(body)
        for name, _state in id_tables.get(ref.group(1), []) if ref else []:
            record(name, cls)
    used = set(table.values())
    if not used <= over:
        die(f"classes outside the override set: {sorted(used - over)}")
    return table, over


def items(method):
    """Item name -> the most-derived class overriding `method` (`useOn` or
    `use`), and the override set; plus the `Items` constant -> name map."""
    use_on = r"public\s+InteractionResult\s+" + method + r"\s*\("
    over = overriding(ITEM_DIR, use_on) - {"Item", "ItemStack"}
    sups = item_superclasses()
    src = read(os.path.join(ITEM_DIR, "Items.java"))
    ids = {}
    for f, kind in (("ItemIds.java", "ItemId"), ("BlockItemIds.java", "BlockItemId")):
        # `create("name")`, or for a block item named apart from its block
        # `BlockItemId.create("wheat", "wheat_seeds")`: the item is the second.
        for const, first, second in re.findall(
                r"\b" + kind + r"\b[^=;]*?\s(\w+)\s*=\s*\w*\.?create\(\"([a-z0-9_]+)\"(?:\s*,\s*\"([a-z0-9_]+)\")?\)",
                read(os.path.join(DECOMP, "references", f))):
            ids[(kind, const)] = "minecraft:" + (second or first)
    known = set()
    reg = json.load(open(os.path.join(os.path.dirname(gbl.REGISTRY), "registries.json"), encoding="utf8"))
    known = set(reg["minecraft:item"]["entries"].keys())
    table = {}
    consts = {}
    unresolved = []
    for m in re.finditer(r"public\s+static\s+final\s+Item\s+([A-Z0-9_]+)\s*=\s*(\w+)\s*\(", src):
        const, call = m.group(1), m.group(2)
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"\b(ItemIds|BlockItemIds)\.(\w+)", body)
        name = None
        if ref:
            name = ids.get(("ItemId" if ref.group(1) == "ItemIds" else "BlockItemId", ref.group(2)))
        name = name or f"minecraft:{const.lower()}"
        if name not in known:
            unresolved.append(name)
            continue
        consts[const] = name
        explicit = re.search(r"\b(\w+Item)::new|\bnew\s+(\w+Item)\s*\(", body)
        if explicit:
            cls = explicit.group(1) or explicit.group(2)
        elif call == "registerBlock" or "createBlockItemWithCustomItemName" in body:
            cls = "BlockItem"
        elif call == "registerSpawnEgg":
            cls = "SpawnEggItem"
        else:
            cls = "Item"
        if cls not in sups and cls not in ("Item", "BlockItem", "SpawnEggItem"):
            unresolved.append(f"{name} ({cls})")
            continue
        c = most_derived(gbp.chain(sups, cls), over, "Item")
        if c:
            table[name] = c
    if unresolved:
        die(f"{len(unresolved)} item declarations did not resolve: {unresolved[:8]}")
    used = set(table.values())
    if not used <= over:
        die(f"classes outside the override set: {sorted(used - over)}")
    return table, over, consts


def compostables(consts):
    """`ComposterBlock.bootStrap`'s `add(chance, Items.X)` registrations."""
    src = read(os.path.join(gbp.BLOCK_DIR, "ComposterBlock.java"))
    found = re.findall(r"\badd\(\s*[\d.]+F\s*,\s*Items\.(\w+)\s*\)", src)
    missing = [c for c in found if c not in consts]
    if missing or len(found) < 100:
        die(f"composter: {len(found)} registrations, unresolved {missing[:5]}")
    return sorted({consts[c] for c in found})


def food_items():
    """Items with a `minecraft:food` component, and those of them whose
    `can_always_eat` is set, from the per-item datagen component reports
    (`Consumable.canConsume` -> `Player.canEat(canAlwaysEat)`)."""
    d = os.path.join(os.path.dirname(gbl.REGISTRY), "minecraft", "components", "item")
    food, always = [], []
    for fn in sorted(os.listdir(d)):
        j = json.load(open(os.path.join(d, fn), encoding="utf8"))
        comps = j.get("components", j)
        f = comps.get("minecraft:food")
        if isinstance(f, dict):
            food.append("minecraft:" + fn[: -len(".json")])
            if f.get("can_always_eat"):
                always.append(food[-1])
    if len(food) < 30 or not always:
        die("too few food components: the component report layout changed")
    return food, always


def main():
    btab, bover = blocks("useItemOn")
    wtab, wover = blocks("useWithoutItem")
    itab, iover, consts = items("useOn")
    utab, uover, _ = items("use")
    comp = compostables(consts)
    food, always = food_items()
    if len(btab) < 20 or len(wtab) < 100 or len(itab) < 400:
        die(f"suspiciously small tables: {len(btab)} blocks, {len(itab)} items")
    out = open(OUT, "w", encoding="utf8", newline="\n")
    w = out.write
    w("//! GENERATED by `tools/gen_click_use.py` — do not edit.\n//!\n")
    w("//! Source: the decompiled 26.2 `Blocks.java`, `Items.java`, the\n")
    w("//! `references/*Ids.java` constants and every block / item class's\n")
    w("//! `extends` chain. Which class's `useWithoutItem` / `useItemOn` (blocks)\n")
    w("//! or `useOn` (items) answers a right-click; the rules are\n")
    w("//! `rewo_world::click_use`.\n\n")
    w(f"/// Block name -> the class overriding `useItemOn` ({len(btab)} blocks).\n")
    w("pub const BLOCK_ITEM_ON_CLASS: &[(&str, &str)] = &[\n")
    for k in sorted(btab):
        w(f"    (\"{k}\", \"{btab[k]}\"),\n")
    w("];\n\n")
    w(f"/// Block name -> the class overriding `useWithoutItem` ({len(wtab)} blocks).\n")
    w("pub const BLOCK_WITHOUT_ITEM_CLASS: &[(&str, &str)] = &[\n")
    for k in sorted(wtab):
        w(f"    (\"{k}\", \"{wtab[k]}\"),\n")
    w("];\n\n")
    w(f"/// Item name -> the overriding `useOn` class ({len(itab)} items).\n")
    w("pub const ITEM_CLICK_CLASS: &[(&str, &str)] = &[\n")
    for k in sorted(itab):
        w(f"    (\"{k}\", \"{itab[k]}\"),\n")
    w("];\n\n")
    w("/// Every block class that overrides `useItemOn`.\n")
    w("pub const BLOCK_ITEM_ON_CLASSES: &[&str] = &[\n")
    for c in sorted(bover):
        w(f"    \"{c}\",\n")
    w("];\n\n")
    w("/// Every block class that overrides `useWithoutItem`.\n")
    w("pub const BLOCK_WITHOUT_ITEM_CLASSES: &[&str] = &[\n")
    for c in sorted(wover):
        w(f"    \"{c}\",\n")
    w("];\n\n")
    w(f"/// Items with a `minecraft:food` component ({len(food)} items).\n")
    w("pub const FOOD: &[&str] = &[\n")
    for k in food:
        w(f"    \"{k}\",\n")
    w("];\n\n")
    w(f"/// Item name -> the overriding `use` class ({len(utab)} items).\n")
    w("pub const ITEM_USE_CLASS: &[(&str, &str)] = &[\n")
    for k in sorted(utab):
        w(f"    (\"{k}\", \"{utab[k]}\"),\n")
    w("];\n\n")
    w(f"/// `ComposterBlock.COMPOSTABLES` ({len(comp)} items).\n")
    w("pub const COMPOSTABLES: &[&str] = &[\n")
    for k in comp:
        w(f"    \"{k}\",\n")
    w("];\n\n")
    w(f"/// Food with `can_always_eat` ({len(always)} items).\n")
    w("pub const ALWAYS_EDIBLE: &[&str] = &[\n")
    for k in always:
        w(f"    \"{k}\",\n")
    w("];\n\n")
    w("/// Every item class that overrides `use`.\n")
    w("pub const ITEM_USE_CLASSES: &[&str] = &[\n")
    for c in sorted(uover):
        w(f"    \"{c}\",\n")
    w("];\n\n")
    w("/// Every item class that overrides `useOn`.\n")
    w("pub const ITEM_USE_ON_CLASSES: &[&str] = &[\n")
    for c in sorted(iover):
        w(f"    \"{c}\",\n")
    w("];\n")
    out.close()
    print(f"gen_click_use: {len(btab)} / {len(wtab)} blocks, {len(itab)} items, {len(food)} food, "
          f"{len(bover)} block classes, {len(iover)} item classes, {len(utab)} use items, "
          f"{len(comp)} compostables, {len(always)} always-edible -> {OUT}")


if __name__ == "__main__":
    main()
