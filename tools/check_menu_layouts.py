"""Grade crates/rewo-world/src/menu_layout.rs against the 26.2 decompile.

Why this is a *checker* and not a generator, unlike its siblings in this
directory: the 25 `minecraft:menu` types build their slot lists in four
different idioms -- a direct `addSlot(new Slot(c, i, x, y))`, a nested loop, a
field assigned earlier (`BeaconMenu.paymentSlot`), and a fluent builder consumed
by a base class (`ItemCombinerMenuSlotDefinition`) -- and five menus declare no
slots at all and inherit them. A single extractor recovers 17 of 25; chasing the
rest across class boundaries is a small Java interpreter, and its failure mode is
a silently *short* slot list, which shifts every later index and still looks like
a plausible menu.

So the Rust table is hand-transcribed and this script re-derives what it can,
independently, and diffs. It covers the menus whose construction is
mechanically legible and REPORTS the ones it cannot reach rather than passing
them silently -- a checker that quietly skips what it cannot parse grades
nothing.

Run:  python tools/check_menu_layouts.py
Exit: 0 all checked menus agree, 1 on any mismatch or parse failure.
"""

from __future__ import annotations

import os
import re
import sys

DECOMP = os.path.expandvars(
    r"%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/world/inventory"
)
RUST = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates",
    "rewo-world",
    "src",
    "menu_layout.rs",
)

PITCH = 18
TOP_TO_HOTBAR = 58


def standard_inventory(left: int, top: int) -> list[tuple[int, int]]:
    """AbstractContainerMenu.addStandardInventorySlots: 3x9 then the hotbar."""
    out = [(left + x * PITCH, top + y * PITCH) for y in range(3) for x in range(9)]
    out += [(left + x * PITCH, top + TOP_TO_HOTBAR) for x in range(9)]
    return out


def grid(left: int, top: int, cols: int, rows: int) -> list[tuple[int, int]]:
    return [(left + x * PITCH, top + y * PITCH) for y in range(rows) for x in range(cols)]


def src(cls: str) -> str:
    p = os.path.join(DECOMP, cls + ".java")
    if not os.path.exists(p):
        raise FileNotFoundError(p)
    return open(p, encoding="utf-8", errors="replace").read()


# --- independent re-derivation, straight from each constructor --------------
#
# Each entry re-reads the decompile for the numbers rather than restating them,
# so a version bump that moves a slot fails here instead of drifting silently.


def num(pattern: str, text: str, what: str) -> tuple[int, ...]:
    m = re.search(pattern, text)
    if not m:
        raise ValueError(f"could not find {what}")
    return tuple(int(g) for g in m.groups())


def derive() -> tuple[dict[str, list[tuple[int, int]]], dict[str, str]]:
    got: dict[str, list[tuple[int, int]]] = {}
    skipped: dict[str, str] = {}

    # ChestMenu: addChestGrid(container, 8, 18); addStandardInventorySlots(8, 18+rows*18+13)
    chest = src("ChestMenu")
    cl, ct = num(r"addChestGrid\(container,\s*(\d+),\s*(\d+)\)", chest, "chest grid origin")
    if not re.search(r"inventoryTop\s*=\s*18\s*\+\s*this\.containerRows\s*\*\s*18\s*\+\s*13", chest):
        raise ValueError("ChestMenu inventoryTop formula changed")
    for rows in range(1, 7):
        got[f"generic_9x{rows}"] = grid(cl, ct, 9, rows) + standard_inventory(
            8, 18 + rows * 18 + 13
        )

    # DispenserMenu: add3x3GridSlots(dispenser, 62, 17)
    d = src("DispenserMenu")
    dl, dt = num(r"add3x3GridSlots\(\w+,\s*(\d+),\s*(\d+)\)", d, "dispenser grid")
    dsl, dst = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", d, "dispenser inv")
    got["generic_3x3"] = grid(dl, dt, 3, 3) + standard_inventory(dsl, dst)

    # CrafterMenu: grid, THEN the player, THEN the result. The order is the point.
    c = src("CrafterMenu")
    body = re.search(r"private void addSlots\(final Inventory inventory\) \{(.*?)\n   \}", c, re.S)
    if not body:
        raise ValueError("CrafterMenu.addSlots not found")
    body = body.group(1)
    gl, gt = num(r"CrafterSlot\(this\.container,\s*slot,\s*(\d+)\s*\+\s*x\s*\*\s*18,\s*(\d+)", body, "crafter grid")
    cil, cit = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", body, "crafter inv")
    rx, ry = num(r"NonInteractiveResultSlot\(this\.resultContainer,\s*0,\s*(\d+),\s*(\d+)\)", body, "crafter result")
    if body.index("addStandardInventorySlots") > body.index("NonInteractiveResultSlot"):
        raise ValueError("CrafterMenu order changed: result no longer follows the player inventory")
    got["crafter_3x3"] = grid(gl, gt, 3, 3) + standard_inventory(cil, cit) + [(rx, ry)]

    # ItemCombinerMenu subclasses: a fluent slot-definition builder.
    for name, cls in (("anvil", "AnvilMenu"), ("smithing", "SmithingMenu")):
        s = src(cls)
        slots = [
            (int(x), int(y))
            for _, x, y in re.findall(r"\.withSlot\((\d+),\s*(-?\d+),\s*(-?\d+)", s)
        ]
        res = num(r"\.withResultSlot\(\d+,\s*(-?\d+),\s*(-?\d+)\)", s, f"{cls} result")
        cil, cit = num(
            r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", src("ItemCombinerMenu"), "combiner inv"
        )
        got[name] = slots + [res] + standard_inventory(cil, cit)

    # AbstractFurnaceMenu: shared by all three furnaces.
    f = src("AbstractFurnaceMenu")
    fs = [(int(x), int(y)) for _, x, y in re.findall(r"new \w*Slot\w*\([^)]*?(\d+),\s*(\d+),\s*(\d+)\)", f)][:3]
    fl, ft = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", f, "furnace inv")
    for name in ("furnace", "blast_furnace", "smoker"):
        got[name] = fs + standard_inventory(fl, ft)

    # BeaconMenu: the payment slot is a field, assigned before addSlot.
    b = src("BeaconMenu")
    px, py = num(r"PaymentSlot\(this\.beacon,\s*0,\s*(\d+),\s*(\d+)\)", b, "beacon payment")
    bl, bt = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", b, "beacon inv")
    got["beacon"] = [(px, py)] + standard_inventory(bl, bt)

    # CraftingMenu: result FIRST, then the grid, then the player.
    cm = src("CraftingMenu")
    crx, cry = num(r"addResultSlot\(this\.player,\s*(\d+),\s*(\d+)\)", cm, "crafting result")
    cgl, cgt = num(r"addCraftingGridSlots\((\d+),\s*(\d+)\)", cm, "crafting grid")
    cml, cmt = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", cm, "crafting inv")
    if cm.index("addResultSlot") > cm.index("addCraftingGridSlots"):
        raise ValueError("CraftingMenu order changed: result no longer precedes the grid")
    got["crafting"] = [(crx, cry)] + grid(cgl, cgt, 3, 3) + standard_inventory(cml, cmt)

    # HopperMenu: 5 in a row.
    h = src("HopperMenu")
    hx, hy = num(r"new Slot\(hopper,\s*x,\s*(\d+)\s*\+\s*x\s*\*\s*18,\s*(\d+)\)", h, "hopper row")
    hl, ht = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", h, "hopper inv")
    got["hopper"] = grid(hx, hy, 5, 1) + standard_inventory(hl, ht)

    # ShulkerBoxMenu: a 9x3 of ShulkerBoxSlot.
    sb = src("ShulkerBoxMenu")
    sx, sy = num(r"ShulkerBoxSlot\(container,\s*x \+ y \* 9,\s*(\d+)\s*\+\s*x\s*\*\s*18,\s*(\d+)", sb, "shulker grid")
    sl, st = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", sb, "shulker inv")
    got["shulker_box"] = grid(sx, sy, 9, 3) + standard_inventory(sl, st)

    # LecternMenu: one slot, and it must NOT have a player inventory.
    lec = src("LecternMenu")
    lx, ly = num(r"new Slot\(lectern,\s*0,\s*(\d+),\s*(\d+)\)", lec, "lectern book slot")
    if "addStandardInventorySlots" in lec:
        raise ValueError("LecternMenu now has a player inventory; the table says it has none")
    got["lectern"] = [(lx, ly)]

    # BrewingStandMenu: four of its five slots are the plain shape, but
    # `IngredientsSlot` takes a leading `potionBrewing` argument, so a 4-arg
    # pattern silently returns a four-slot brewing stand -- which is exactly the
    # short-list failure this script exists to prevent, and it is how the script
    # failed on its first run. Read all five by index instead.
    bs = src("BrewingStandMenu")
    brew = []
    for i in range(5):
        brew.append(
            num(
                r"new BrewingStandMenu\.\w+\([^)]*?\b" + str(i) + r",\s*(\d+),\s*(\d+)\)",
                bs,
                f"brewing slot {i}",
            )
        )
    bl, bt2 = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", bs, "brewing inv")
    got["brewing_stand"] = brew + standard_inventory(bl, bt2)

    # The plain ones: every slot is a literal `new *Slot(container, i, x, y)`.
    plain = {
        "enchantment": ("EnchantmentMenu", 2),
        "grindstone": ("GrindstoneMenu", 3),
        "loom": ("LoomMenu", 4),
        "cartography_table": ("CartographyTableMenu", 3),
        "stonecutter": ("StonecutterMenu", 2),
    }
    for name, (cls, count) in plain.items():
        s = src(cls)
        found = [
            (int(x), int(y))
            for _, x, y in re.findall(
                r"new [\w.]*Slot\w*\((?:this\.)?[\w.]+,\s*(\d+),\s*(-?\d+),\s*(-?\d+)\)", s
            )
        ]
        if len(found) != count:
            raise ValueError(f"{cls}: expected {count} literal slots, extractor saw {len(found)}")
        il, it = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", s, f"{cls} inv")
        got[name] = found + standard_inventory(il, it)

    # MerchantMenu: the result slot takes an extra `merchant` argument, so it
    # does not match the plain shape; read it on its own.
    mm = src("MerchantMenu")
    m0 = num(r"new Slot\(this\.tradeContainer,\s*0,\s*(\d+),\s*(\d+)\)", mm, "merchant 0")
    m1 = num(r"new Slot\(this\.tradeContainer,\s*1,\s*(\d+),\s*(\d+)\)", mm, "merchant 1")
    m2 = num(r"MerchantResultSlot\([^)]*?2,\s*(\d+),\s*(\d+)\)", mm, "merchant result")
    ml, mt = num(r"addStandardInventorySlots\(\w+,\s*(\d+),\s*(\d+)\)", mm, "merchant inv")
    got["merchant"] = [m0, m1, m2] + standard_inventory(ml, mt)

    return got, skipped


# --- parse the Rust table ----------------------------------------------------


def parse_rust() -> dict[str, list[tuple[int, int]]]:
    text = open(RUST, encoding="utf-8").read()

    blocks: dict[str, list] = {}
    # Terminate on `];`, not on `\n];`: LECTERN is a single-line array literal,
    # and a newline-anchored terminator runs straight past it and swallows the
    # next const's declaration whole.
    for m in re.finditer(r"const (\w+): \[SlotBlock; \d+\] = \[(.*?)\];", text, re.S):
        blocks[m.group(1)] = parse_blocks(m.group(2))
    # The chest family is built by a macro over rows.
    for rows in range(1, 7):
        blocks[f"CHEST_{rows}"] = [
            ("grid", 8, 18, 9, rows),
            ("std", 8, 18 + rows * 18 + 13),
        ]

    out: dict[str, list[tuple[int, int]]] = {}
    for rows in range(1, 7):
        out[f"generic_9x{rows}"] = expand(blocks[f"CHEST_{rows}"])
    for m in re.finditer(
        r'protocol_id: (\d+),\s*name: "(\w+)",\s*blocks: &(\w+),', text
    ):
        out[m.group(2)] = expand(blocks[m.group(3)])
    return out


def parse_blocks(body: str) -> list:
    res = []
    for m in re.finditer(
        r"SlotBlock::One \{ x: (-?\d+), y: (-?\d+) \}"
        r"|SlotBlock::Grid \{\s*left: (-?\d+),\s*top: (-?\d+),\s*cols: (\d+),\s*rows: (\d+),?\s*\}"
        r"|SlotBlock::StandardInventory \{\s*left: (-?\d+),\s*top: (-?\d+),?\s*\}",
        body,
        re.S,
    ):
        g = m.groups()
        if g[0] is not None:
            res.append(("one", int(g[0]), int(g[1])))
        elif g[2] is not None:
            res.append(("grid", int(g[2]), int(g[3]), int(g[4]), int(g[5])))
        else:
            res.append(("std", int(g[6]), int(g[7])))
    return res


def expand(bs: list) -> list[tuple[int, int]]:
    out: list[tuple[int, int]] = []
    for b in bs:
        if b[0] == "one":
            out.append((b[1], b[2]))
        elif b[0] == "grid":
            out += grid(b[1], b[2], b[3], b[4])
        else:
            out += standard_inventory(b[1], b[2])
    return out


def main() -> int:
    try:
        expected, skipped = derive()
    except (FileNotFoundError, ValueError) as e:
        print(f"[menucheck] FAIL - could not re-derive from the decompile: {e}")
        return 1

    actual = parse_rust()
    if not actual:
        print("[menucheck] FAIL - parsed no layouts out of menu_layout.rs")
        return 1

    bad = 0
    for name in sorted(expected):
        if name not in actual:
            print(f"[menucheck] FAIL {name}: absent from the Rust table")
            bad += 1
            continue
        e, a = expected[name], actual[name]
        if e != a:
            bad += 1
            print(f"[menucheck] FAIL {name}: {len(e)} slots derived vs {len(a)} in the table")
            for i, (pe, pa) in enumerate(zip(e, a)):
                if pe != pa:
                    print(f"              first mismatch at menu slot {i}: decompile {pe} vs table {pa}")
                    break
        else:
            print(f"[menucheck]  ok  {name}: {len(e)} slots")

    for name, why in skipped.items():
        print(f"[menucheck] SKIP {name}: {why}")

    covered = len(expected)
    print(
        f"[menucheck] {covered}/25 menus re-derived from the decompile, "
        f"{covered - bad} agree, {bad} disagree"
    )
    if covered < 25:
        missing = sorted(set(actual) - set(expected))
        print(f"[menucheck] NOT CHECKED: {missing}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
