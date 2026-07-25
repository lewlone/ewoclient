"""Machine-extract vanilla's per-item **default** `minecraft:swing_animation`
component into a Rust table (`crates/rewo-data/src/swing_anim_table.rs`).

Why this exists
---------------
`LivingEntity.getCurrentSwingDuration()` reads
`getItemInHand(hand).getSwingAnimation().duration()`, and
`ArmedEntityRenderState` reads `.type()`. Both come from the item stack's
`minecraft:swing_animation` component.

That component is almost never on the wire. `ItemStack.OPTIONAL_STREAM_CODEC`
sends `count + item-registry-id + DataComponentPatch`, and the patch carries
only *deltas from the item's prototype*. `DataComponents.COMMON_ITEM_COMPONENTS`
sets `SWING_ANIMATION -> SwingAnimation.DEFAULT` on every item, and the seven
spears override it in their own item properties — so the client resolves the
value from the **item id**, not from the packet. That mapping is vanilla data,
and this script is where it comes from.

Ground truth is the datagen item-component report, never community docs
(REWO_PLAN §11):

    <APPDATA>/EwoClient/rewo/<version>/datagen/generated/reports/
        minecraft/components/item/<item>.json

plus two decompiled classes, read to pin the *meaning* of the numbers:

    net/minecraft/world/item/component/SwingAnimation.java   (the DEFAULT record)
    net/minecraft/world/item/SwingAnimationType.java         (the enum ids)

Re-run after a version bump:

    python tools/gen_swing_animations.py

Fails loud rather than defaulting: an unknown key inside a `swing_animation`
object, an unparseable `SwingAnimation.DEFAULT`, an enum whose ids are not the
ones the wire codec uses, a missing report tree, or an item file that carries no
`swing_animation` at all are all hard errors. A version bump that changes any of
those stops here instead of silently shipping a wrong swing duration.
"""
import json
import os
import re
import sys

VERSION = "26.2"
ROOT = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", VERSION)
REPORT = os.path.join(ROOT, "datagen", "generated", "reports",
                      "minecraft", "components", "item")
DECOMP = os.path.join(ROOT, "decompiled", "net", "minecraft", "world", "item")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "swing_anim_table.rs")

KEY = "minecraft:swing_animation"


def die(msg):
    print(f"gen_swing_animations: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    with open(path, "r", encoding="utf-8") as f:
        return f.read()


def enum_ids():
    """`SwingAnimationType` -> {serialized name: wire id}, from the decompile.

    The enum declares `NONE(0, "none")` etc. and its `STREAM_CODEC` is
    `ByteBufCodecs.idMapper(BY_ID, getId)`, so the declared int *is* the wire id.
    """
    src = read(os.path.join(DECOMP, "SwingAnimationType.java"))
    if "ByteBufCodecs.idMapper(BY_ID, SwingAnimationType::getId)" not in src:
        die("SwingAnimationType.STREAM_CODEC is no longer idMapper(getId) — "
            "the wire id may not be the declared int any more")
    out = {}
    for name, num, ser in re.findall(
            r'\b([A-Z_]+)\(\s*(\d+)\s*,\s*"([a-z_]+)"\s*\)', src):
        out[ser] = (name, int(num))
    if set(out) != {"none", "whack", "stab"}:
        die(f"unexpected SwingAnimationType members: {sorted(out)}")
    return out


def default_swing():
    """`SwingAnimation.DEFAULT` -> (serialized type name, duration)."""
    src = read(os.path.join(DECOMP, "component", "SwingAnimation.java"))
    m = re.search(
        r'DEFAULT\s*=\s*new SwingAnimation\(\s*SwingAnimationType\.([A-Z_]+)\s*,\s*(\d+)\s*\)',
        src)
    if not m:
        die("could not parse SwingAnimation.DEFAULT from the decompile")
    return m.group(1), int(m.group(2))


def main():
    if not os.path.isdir(REPORT):
        die(f"missing item-component report at {REPORT}")
    ids = enum_ids()                      # serialized name -> (JAVA_NAME, id)
    by_java = {j: (s, i) for s, (j, i) in ids.items()}
    def_java, def_duration = default_swing()
    if def_java not in by_java:
        die(f"SwingAnimation.DEFAULT names unknown type {def_java}")
    def_serialized, def_id = by_java[def_java]

    files = sorted(f for f in os.listdir(REPORT) if f.endswith(".json"))
    if not files:
        die(f"no item reports under {REPORT}")

    non_default = []
    missing = []
    for fname in files:
        item = "minecraft:" + fname[:-len(".json")]
        data = json.loads(read(os.path.join(REPORT, fname)))
        comps = data.get("components")
        if comps is None:
            die(f"{fname}: no 'components' object")
        if KEY not in comps:
            # Every 26.2 item inherits COMMON_ITEM_COMPONENTS, so an item
            # without the key means the component moved — fail rather than
            # silently assuming the default.
            missing.append(item)
            continue
        obj = comps[KEY]
        if not isinstance(obj, dict):
            die(f"{item}: {KEY} is {type(obj).__name__}, expected an object")
        unknown = set(obj) - {"type", "duration"}
        if unknown:
            die(f"{item}: unknown {KEY} keys {sorted(unknown)} — the record grew")
        ty = obj.get("type", def_serialized)
        dur = obj.get("duration", def_duration)
        if ty not in ids:
            die(f"{item}: unknown swing animation type {ty!r}")
        if not isinstance(dur, int) or dur <= 0:
            die(f"{item}: non-positive duration {dur!r} (codec is POSITIVE_INT)")
        if (ty, dur) != (def_serialized, def_duration):
            non_default.append((item, ids[ty][1], dur))

    if missing:
        die(f"{len(missing)} item(s) carry no {KEY} (e.g. {missing[:3]}) — "
            "COMMON_ITEM_COMPONENTS no longer sets it")

    with open(OUT, "w", encoding="utf-8", newline="\n") as out:
        out.write("//! GENERATED by `tools/gen_swing_animations.py` — do not edit.\n")
        out.write("//!\n")
        out.write(f"//! Source: the {VERSION} datagen item-component report\n")
        out.write("//! (`reports/minecraft/components/item/*.json`), field\n")
        out.write(f"//! `{KEY}`, cross-checked against the decompiled\n")
        out.write("//! `SwingAnimation.DEFAULT` and `SwingAnimationType` wire ids.\n")
        out.write("//!\n")
        out.write("//! The component is a *prototype* value: `DataComponents`\n")
        out.write("//! `COMMON_ITEM_COMPONENTS` sets it on every item, so it is not in the\n")
        out.write("//! `DataComponentPatch` a server sends — the client resolves it from the\n")
        out.write("//! item id. That is why this table exists.\n")
        out.write("//!\n")
        out.write(f"//! {len(files)} items scanned, {len(non_default)} non-default.\n")
        out.write("\n")
        out.write("/// Items present in the report this table was generated from. A runtime\n")
        out.write("/// registry with a wildly different size means the pin has drifted.\n")
        out.write(f"pub const SCANNED_ITEMS: usize = {len(files)};\n\n")
        out.write("/// `SwingAnimation.DEFAULT.type()` as its wire id\n")
        out.write(f"/// (`SwingAnimationType.{def_java}`).\n")
        out.write(f"pub const DEFAULT_TYPE_ID: u8 = {def_id};\n\n")
        out.write("/// `SwingAnimation.DEFAULT.duration()`, in ticks.\n")
        out.write(f"pub const DEFAULT_DURATION: i32 = {def_duration};\n\n")
        out.write("/// Every item whose prototype `swing_animation` differs from the default:\n")
        out.write("/// `(registry name, SwingAnimationType wire id, duration ticks)`.\n")
        out.write("pub const NON_DEFAULT: &[(&str, u8, i32)] = &[\n")
        for item, tid, dur in sorted(non_default):
            out.write(f'    ("{item}", {tid}, {dur}),\n')
        out.write("];\n")
    print(f"gen_swing_animations: {len(files)} items scanned, "
          f"{len(non_default)} non-default -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
