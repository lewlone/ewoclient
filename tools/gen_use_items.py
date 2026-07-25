"""Machine-extract vanilla's per-item **use duration** and **use animation**
into a Rust table (`crates/rewo-data/src/use_item_table.rs`).

Why this exists
---------------
`AvatarRenderer.getArmPose` selects eight of the eleven `HumanoidModel.ArmPose`
values from `itemInHand.getUseAnimation()`, gated on
`getUseItemRemainingTicks() > 0`. That remaining-tick counter is **not** on the
wire — `LivingEntity.onSyncedDataUpdated` derives it client-side the moment the
`DATA_LIVING_ENTITY_FLAGS` "using" bit flips on:

    this.useItem = this.getItemInHand(this.getUsedItemHand());
    this.useItemRemaining = this.useItem.getUseDuration(this);

So the client needs `getUseDuration` and `getUseAnimation` resolved from the
item id, exactly as it needs `swing_animation` (see `gen_swing_animations.py`).
That mapping is vanilla data, and this script is where it comes from.

Two sources, both ground truth (REWO_PLAN §11 — never community docs):

1. The datagen item-component report, for the base rule's inputs::

       <APPDATA>/EwoClient/rewo/<version>/datagen/generated/reports/
           minecraft/components/item/<item>.json

   `Item.getUseAnimation` (decompiled `world/item/Item.java`)::

       Consumable c = stack.get(CONSUMABLE);
       if (c != null)                    return c.animation();
       if (stack.has(BLOCKS_ATTACKS))    return BLOCK;
       return stack.has(KINETIC_WEAPON) ? SPEAR : NONE;

   `Item.getUseDuration`::

       Consumable c = stack.get(CONSUMABLE);
       if (c != null) return c.consumeTicks();      // (int)(consumeSeconds * 20)
       return (has(BLOCKS_ATTACKS) || has(KINETIC_WEAPON)) ? 72000 : 0;

2. The decompile, for the eight item classes that override either method, and
   for the `ItemUseAnimation` wire ids::

       net/minecraft/world/item/{Bow,Brush,Bundle,Crossbow,EnderEye,
                                 Instrument,Spyglass,Trident}Item.java
       net/minecraft/world/item/ItemUseAnimation.java
       net/minecraft/world/item/Instruments.java   (InstrumentItem's duration)
       net/minecraft/world/item/Items.java         (item id -> class)
       net/minecraft/references/ItemIds.java       (constant -> registry name)

`InstrumentItem.getUseDuration` is the one non-literal override::

    return getInstrument(stack).map(h -> Mth.floor(h.value().useDuration() * 20.0F)).orElse(0);

`Instruments.java` registers every instrument with an explicit `useDuration`
literal, so the value is exact rather than guessed — but only while they all
agree. If a future version gives instruments differing durations, resolving one
would need the synced `minecraft:instrument` registry, which this client does
not parse; the script fails loud in that case rather than picking one.

Re-run after a version bump:

    python tools/gen_use_items.py

Fails loud rather than defaulting: an unknown `ItemUseAnimation` member, an
override method whose body is no longer a bare literal, an override class that
is registered under a name absent from the report, a `consumable` object with an
unknown key, a non-numeric `consume_seconds`, disagreeing instrument durations,
or a missing report tree. A version bump that changes any of those stops here
instead of silently shipping a wrong arm pose.
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
ITEM_DIR = os.path.join(DECOMP, "world", "item")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "use_item_table.rs")

CONSUMABLE = "minecraft:consumable"
BLOCKS_ATTACKS = "minecraft:blocks_attacks"
KINETIC_WEAPON = "minecraft:kinetic_weapon"

# `Consumable.DEFAULT_CONSUME_SECONDS` / the codec's `optionalFieldOf` defaults.
DEFAULT_CONSUME_SECONDS = 1.6
DEFAULT_CONSUME_ANIMATION = "eat"

# `Item.getUseDuration`'s non-consumable branch.
BLOCKING_DURATION = 72000

# `ColorCollection.NAMES`, in declaration order — the same list
# `gen_block_light.py` uses to expand `createSimpleColored`.
DYE_COLORS = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink",
    "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red",
    "black",
]

# The eight classes that override `getUseDuration` and/or `getUseAnimation`.
# Listed here so that a *new* override appearing in a later version is caught:
# the script re-derives this set from the decompile and compares.
OVERRIDE_CLASSES = [
    "BowItem", "BrushItem", "BundleItem", "CrossbowItem",
    "EnderEyeItem", "InstrumentItem", "SpyglassItem", "TridentItem",
]


def die(msg):
    print(f"gen_use_items: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        die(f"cannot read {path}: {e}")


def animation_ids():
    """`ItemUseAnimation` -> {serialized name: wire id}, from the decompile.

    The enum declares `EAT(1, "eat", true)` etc. and its `STREAM_CODEC` is
    `ByteBufCodecs.idMapper(BY_ID, getId)`, so the declared int *is* the wire
    id — the same reasoning `gen_swing_animations.py` applies to
    `SwingAnimationType`.
    """
    src = read(os.path.join(ITEM_DIR, "ItemUseAnimation.java"))
    if "ByteBufCodecs.idMapper(BY_ID, ItemUseAnimation::getId)" not in src:
        die("ItemUseAnimation.STREAM_CODEC is no longer idMapper(getId) — "
            "the wire id may not be the declared int any more")
    out = {}
    for name, num, ser in re.findall(
            r'\b([A-Z_]+)\(\s*(\d+)\s*,\s*"([a-z_]+)"(?:\s*,\s*true)?\s*\)', src):
        out[ser] = (name, int(num))
    expected = {"none", "eat", "drink", "block", "bow", "trident", "crossbow",
                "spyglass", "toot_horn", "brush", "bundle", "spear"}
    if set(out) != expected:
        die(f"unexpected ItemUseAnimation members: {sorted(out)}")
    return out


def instrument_ticks():
    """`InstrumentItem.getUseDuration` -> ticks, from `Instruments.java`.

    The override is `Mth.floor(instrument.useDuration() * 20.0F)`. Every
    `register(context, KEY, sound, <useDuration>, <range>)` call carries the
    literal. All instruments must agree, because the item component names
    *which* instrument and this client does not parse that registry.
    """
    src = read(os.path.join(ITEM_DIR, "Instruments.java"))
    durations = {
        float(d) for d in
        re.findall(r'register\(\s*context,\s*\w+,\s*\(?[^,]+?\)?[^,]*,\s*'
                   r'([0-9.]+)F\s*,\s*[0-9.]+F\s*\)', src)
    }
    if not durations:
        die("could not parse any instrument useDuration literal from "
            "Instruments.java")
    if len(durations) > 1:
        die(f"instruments no longer share one useDuration ({sorted(durations)}) "
            "— resolving TOOT_HORN would need the synced instrument registry")
    seconds = durations.pop()
    return int(seconds * 20.0)          # Mth.floor of a positive product


def override_table(anims):
    """The eight overriding classes -> `(duration | None, animation | None)`.

    `None` means "does not override that method", so the base component rule
    still applies to it.
    """
    found = {}
    for path in sorted(os.listdir(ITEM_DIR)):
        if not path.endswith("Item.java"):
            continue
        cls = path[:-len(".java")]
        if cls == "Item":
            # The base class *declares* both methods; it does not override them.
            continue
        src = read(os.path.join(ITEM_DIR, path))
        dur = None
        anim = None
        m = re.search(r'public int getUseDuration\([^)]*\)\s*\{\s*'
                      r'return\s+(-?\d+);\s*\}', src)
        if m:
            dur = int(m.group(1))
        elif re.search(r'public int getUseDuration\(', src):
            if cls != "InstrumentItem":
                die(f"{cls}.getUseDuration is no longer a bare literal — "
                    "its value cannot be resolved without new handling")
            dur = instrument_ticks()
        m = re.search(r'public ItemUseAnimation getUseAnimation\([^)]*\)\s*\{\s*'
                      r'return\s+ItemUseAnimation\.([A-Z_]+);\s*\}', src)
        if m:
            java = m.group(1)
            hit = [ser for ser, (j, _) in anims.items() if j == java]
            if not hit:
                die(f"{cls} returns unknown ItemUseAnimation.{java}")
            anim = hit[0]
        elif re.search(r'public ItemUseAnimation getUseAnimation\(', src):
            die(f"{cls}.getUseAnimation is no longer a bare literal")
        if dur is not None or anim is not None:
            found[cls] = (dur, anim)
    # `Item.java` and `ItemStack.java` define/forward rather than override.
    found.pop("Item", None)
    if sorted(found) != sorted(OVERRIDE_CLASSES):
        die(f"the set of use-method overrides changed: {sorted(found)} "
            f"!= {sorted(OVERRIDE_CLASSES)}")
    return found


def item_ids():
    """`ItemIds` constant -> the registry names it stands for."""
    src = read(os.path.join(DECOMP, "references", "ItemIds.java"))
    out = {}
    for const, name in re.findall(
            r'ResourceKey<Item>\s+(\w+)\s*=\s*create\("([a-z0-9_]+)"\)', src):
        out[const] = ["minecraft:" + name]
    for const, base in re.findall(
            r'ColorCollection<ResourceKey<Item>>\s+(\w+)\s*=\s*'
            r'createSimpleColored\("([a-z0-9_]+)"\)', src):
        out[const] = [f"minecraft:{c}_{base}" for c in DYE_COLORS]
    if not out:
        die("could not parse any ItemIds constant")
    return out


def item_classes(ids):
    """Registry name -> overriding item class, from `Items.java`.

    Two registration shapes carry a class: the direct
    `registerItem(ItemIds.X, FooItem::new, ...)` and the colour-collection
    `registerItems(ItemIds.X, (name, var1) -> registerItem(name, FooItem::new, ...))`.
    Only names that map to one of the override classes are kept.
    """
    src = read(os.path.join(ITEM_DIR, "Items.java"))
    out = {}
    for const, cls in re.findall(
            r'register(?:Item|Items)\(\s*ItemIds\.(\w+),\s*'
            r'(?:\([^)]*\)\s*->\s*registerItem\(\s*\w+,\s*)?(\w+)::new', src):
        if cls not in OVERRIDE_CLASSES:
            continue
        if const not in ids:
            die(f"Items.java registers ItemIds.{const} ({cls}) but ItemIds "
                "declares no such constant")
        for name in ids[const]:
            out[name] = cls
    missing = set(OVERRIDE_CLASSES) - set(out.values())
    if missing:
        die(f"no item registers {sorted(missing)} — the registration shape "
            "in Items.java changed")
    return out


def base_rule(item, comps, anims):
    """`Item.getUseDuration` / `getUseAnimation`, from prototype components."""
    if CONSUMABLE in comps:
        obj = comps[CONSUMABLE]
        if not isinstance(obj, dict):
            die(f"{item}: {CONSUMABLE} is {type(obj).__name__}, expected object")
        unknown = set(obj) - {"consume_seconds", "animation", "sound",
                              "has_consume_particles", "on_consume_effects"}
        if unknown:
            die(f"{item}: unknown {CONSUMABLE} keys {sorted(unknown)} — "
                "the record grew")
        secs = obj.get("consume_seconds", DEFAULT_CONSUME_SECONDS)
        if not isinstance(secs, (int, float)) or isinstance(secs, bool):
            die(f"{item}: consume_seconds is {secs!r}, expected a number")
        anim = obj.get("animation", DEFAULT_CONSUME_ANIMATION)
        if anim not in anims:
            die(f"{item}: unknown consumable animation {anim!r}")
        # `Consumable.consumeTicks()` = `(int)(consumeSeconds * 20.0F)` — a
        # Java float-to-int cast, i.e. truncation toward zero.
        return int(secs * 20.0), anim
    if BLOCKS_ATTACKS in comps:
        return BLOCKING_DURATION, "block"
    if KINETIC_WEAPON in comps:
        return BLOCKING_DURATION, "spear"
    return 0, "none"


def main():
    if not os.path.isdir(REPORT):
        die(f"missing item-component report at {REPORT}")
    anims = animation_ids()
    overrides = override_table(anims)
    classes = item_classes(item_ids())

    files = sorted(f for f in os.listdir(REPORT) if f.endswith(".json"))
    if not files:
        die(f"no item reports under {REPORT}")

    known = {"minecraft:" + f[:-len(".json")] for f in files}
    for name in classes:
        if name not in known:
            die(f"{name} is registered with an override class but has no "
                "component report — the ItemIds expansion is wrong")

    rows = []
    for fname in files:
        item = "minecraft:" + fname[:-len(".json")]
        data = json.loads(read(os.path.join(REPORT, fname)))
        comps = data.get("components")
        if comps is None:
            die(f"{fname}: no 'components' object")
        dur, anim = base_rule(item, comps, anims)
        cls = classes.get(item)
        if cls is not None:
            o_dur, o_anim = overrides[cls]
            if o_dur is not None:
                dur = o_dur
            if o_anim is not None:
                anim = o_anim
        if dur < 0:
            die(f"{item}: negative use duration {dur}")
        if (dur, anim) != (0, "none"):
            rows.append((item, dur, anims[anim][1], anim))

    with open(OUT, "w", encoding="utf-8", newline="\n") as out:
        out.write("//! GENERATED by `tools/gen_use_items.py` — do not edit.\n")
        out.write("//!\n")
        out.write(f"//! Source: the {VERSION} datagen item-component report\n")
        out.write("//! (`reports/minecraft/components/item/*.json`) resolved through\n")
        out.write("//! decompiled `Item.getUseDuration` / `getUseAnimation`, plus the\n")
        out.write("//! eight item classes that override either method.\n")
        out.write("//!\n")
        out.write("//! Like `swing_anim_table`, this is a *prototype* resolution: the values\n")
        out.write("//! come from the item id, because the components they read are not in the\n")
        out.write("//! `DataComponentPatch` a server sends for an ordinary stack.\n")
        out.write("//!\n")
        out.write(f"//! {len(files)} items scanned, {len(rows)} usable.\n")
        out.write("\n")
        out.write("/// Items present in the report this table was generated from.\n")
        out.write(f"pub const SCANNED_ITEMS: usize = {len(files)};\n\n")
        out.write("/// `ItemUseAnimation` wire ids, as declared by the enum.\n")
        out.write("pub mod anim {\n")
        for ser, (java, wid) in sorted(anims.items(), key=lambda kv: kv[1][1]):
            out.write(f"    /// `ItemUseAnimation.{java}`\n")
            out.write(f"    pub const {java}: u8 = {wid};\n")
        out.write("}\n\n")
        out.write("/// `Item.getUseDuration`'s non-consumable branch, in ticks —\n")
        out.write("/// effectively \"until released\".\n")
        out.write(f"pub const BLOCKING_DURATION: i32 = {BLOCKING_DURATION};\n\n")
        out.write("/// Every item that can actually be used: `(registry name, use duration\n")
        out.write("/// ticks, ItemUseAnimation wire id)`. An item absent from this table\n")
        out.write("/// resolves to `(0, NONE)` — it cannot be used, so it can never reach a\n")
        out.write("/// use-driven arm pose.\n")
        out.write("pub const USABLE: &[(&str, i32, u8)] = &[\n")
        for item, dur, wid, ser in sorted(rows):
            out.write(f'    ("{item}", {dur}, {wid}), // {ser}\n')
        out.write("];\n")
    print(f"gen_use_items: {len(files)} items scanned, {len(rows)} usable "
          f"-> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
