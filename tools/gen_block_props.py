#!/usr/bin/env python3
"""Extract per-block render-culling and movement facts from the decompiled jar.

Companion to `gen_block_light.py` (and reuses its registration parser): the
facts here live in `Blocks.java` builder calls, block-class overrides and block
tags rather than in any datagen report.

Emitted into `crates/rewo-data/src/block_props.rs`:

* `SKIP_RENDERING` — `Block.skipRendering` overrides, by class chain:
  `HalfTransparentBlock` / `PowderSnowBlock` skip a same-block neighbour (1),
  `IronBarsBlock` has the connected-pane rule (2), `MangroveRootsBlock` skips a
  same-block neighbour on the Y axis only (3). `LeavesBlock` skips only when
  the `cutoutLeaves` option is off, and its default is on, so it is absent.
* `FRICTION` / `SPEED_FACTOR` / `JUMP_FACTOR` / `BOUNCE` — `Properties.friction(..)` etc.
* `STUCK` — `entityInside` → `makeStuckInBlock` multipliers, by class chain.
* `TRAPDOORS` / `LADDERS` / `FENCE_GATES` — class-chain membership for
  `LivingEntity.onClimbable` and `Entity.getOnPos`.
* `CLIMBABLE` / `BARS` / `FENCES` / `WALLS` / `SUPPRESSES_BOUNCE` — block tags, fully expanded.

Run after a version bump:
    python tools/gen_block_props.py
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_block_light as gbl  # noqa: E402

TAGS = os.path.join(gbl.ROOT, "data", "minecraft", "tags", "block")
BLOCK_DIR = os.path.join(gbl.ROOT, "net/minecraft/world/level/block")

SKIP_CLASSES = [
    ("HalfTransparentBlock", 1),
    ("PowderSnowBlock", 1),
    ("IronBarsBlock", 2),
    ("MangroveRootsBlock", 3),
]
# (class, (x, y, z)) — the literal `makeStuckInBlock` multiplier each
# `entityInside` override passes (WebBlock's no-weaving arm).
STUCK_CLASSES = [
    ("WebBlock", "web"),
    ("SweetBerryBushBlock", "sweet_berry_bush"),
    ("PowderSnowBlock", "powder_snow"),
]


def superclasses():
    out = {}
    for dirpath, _dirs, files in os.walk(BLOCK_DIR):
        for fn in files:
            if not fn.endswith(".java"):
                continue
            text = open(os.path.join(dirpath, fn), encoding="utf8", errors="replace").read()
            m = re.search(r"class\s+(\w+)(?:<[^>]*>)?\s+extends\s+(\w+)", text)
            if m:
                out[m.group(1)] = m.group(2)
    return out


def chain(sups, cls):
    seen = []
    while cls and cls not in seen:
        seen.append(cls)
        cls = sups.get(cls)
    return seen


def stuck_multipliers():
    """class -> (x, y, z) literal, read from each override's source."""
    out = {}
    for cls, _ in STUCK_CLASSES:
        text = open(os.path.join(BLOCK_DIR, cls + ".java"), encoding="utf8").read()
        m = re.search(r"new Vec3\(([-\d.F]+),\s*([-\d.F]+),\s*([-\d.F]+)\)", text)
        if not m:
            sys.exit(f"no makeStuckInBlock multiplier in {cls}")
        out[cls] = tuple(m.group(i) for i in (1, 2, 3))
    return out


def expand_tag(name, seen=None):
    seen = seen or set()
    if name in seen:
        return set()
    seen.add(name)
    path = os.path.join(TAGS, name + ".json")
    vals = json.load(open(path, encoding="utf8"))["values"]
    out = set()
    for v in vals:
        v = v["id"] if isinstance(v, dict) else v
        if v.startswith("#"):
            out |= expand_tag(v[1:].split(":", 1)[-1], seen)
        else:
            out.add(v.split(":", 1)[-1])
    return out


def float_prop(expanded, name):
    m = re.search(r"\." + name + r"\(\s*([\d.]+)F\s*\)", expanded)
    return m.group(1) if m else None


def java_float(lit):
    """A Java float literal, spelled so Rust parses the same f32."""
    return lit.rstrip("F") if "." in lit else lit.rstrip("F") + ".0"


def main():
    src = open(gbl.BLOCKS, encoding="utf8", errors="replace").read()
    sups = superclasses()
    id_tables = gbl.scan_id_tables()
    single_ids = gbl.scan_single_ids()
    known = set(json.load(open(gbl.REGISTRY, encoding="utf8")).keys())
    stuck_lit = stuck_multipliers()

    helpers = {}
    for m in re.finditer(r"(?:private|public)\s+static\s+BlockBehaviour\.Properties\s+(\w+)\s*\(", src):
        brace = src.find("{", m.end())
        depth, j = 0, brace
        while j < len(src):
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        helpers[m.group(1)] = src[brace : j + 1]

    skip, friction, speed, jump, bounce, stuck = {}, {}, {}, {}, {}, {}
    trapdoors, ladders, gates = set(), set(), set()
    missing = []

    def record(name, body, cls):
        if f"minecraft:{name}" not in known:
            missing.append(name)
            return
        expanded = body
        for hname, hbody in helpers.items():
            if hname + "(" in expanded:
                expanded += "\n" + hbody
        ch = chain(sups, cls) if cls else []
        for c, kind in SKIP_CLASSES:
            if c in ch:
                skip[name] = kind
                break
        for c, _ in STUCK_CLASSES:
            if c in ch:
                stuck[name] = stuck_lit[c]
                break
        if "TrapDoorBlock" in ch:
            trapdoors.add(name)
        if "LadderBlock" in ch:
            ladders.add(name)
        if "FenceGateBlock" in ch:
            gates.add(name)
        for prop, table in (("friction", friction), ("speedFactor", speed), ("jumpFactor", jump), ("bounceRestitution", bounce)):
            v = float_prop(body, prop) or float_prop(expanded, prop)
            if v is not None:
                table[name] = v

    for m in re.finditer(r"public\s+static\s+final\s+Block\s+([A-Z0-9_]+)\s*=\s*\w+\s*\(", src, re.M):
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"Block(?:Item)?Ids\.(\w+)", body)
        name = single_ids.get(ref.group(1)) if ref else None
        record(name or m.group(1).lower(), body, gbl.impl_class(body))

    for m in re.finditer(
        r"public\s+static\s+final\s+(?:Color|WeatheringCopper)Collection<Block>\s+"
        r"[A-Z0-9_]+\s*=\s*\w+\s*\.\s*registerBlocks\s*\(", src, re.M
    ):
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"BlockItemIds\.(\w+)", body)
        names = id_tables.get(ref.group(1)) if ref else None
        if not names:
            continue
        cls = gbl.impl_class(body)
        for name, _state in names:
            record(name, body, cls)

    if missing:
        print(f"  {len(missing)} names not in the registry: {missing[:5]}", file=sys.stderr)

    tags = {t: sorted(expand_tag(t)) for t in ("climbable", "bars", "fences", "walls", "suppresses_bounce")}

    dest = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "crates", "rewo-data", "src", "block_props.rs",
    )
    out = open(dest, "w", encoding="utf8", newline="\n")
    w = out.write
    w("//! GENERATED by `tools/gen_block_props.py` — do not edit.\n//!\n")
    w(f"//! Source: decompiled {gbl.VERSION} `Blocks.java`, block classes and\n")
    w("//! `data/minecraft/tags/block`. See the script header for the rules.\n\n")

    def str_list(title, doc, items):
        w(f"/// {doc}\npub const {title}: &[&str] = &[\n")
        for k in sorted(items):
            w(f'    "minecraft:{k}",\n')
        w("];\n\n")

    def f32_table(title, doc, table):
        w(f"/// {doc}\npub const {title}: &[(&str, f32)] = &[\n")
        for k in sorted(table):
            w(f'    ("minecraft:{k}", {java_float(table[k])}),\n')
        w("];\n\n")

    w("/// `Block.skipRendering` override kind: 1 = a same-block neighbour,\n")
    w("/// 2 = `IronBarsBlock`'s connected rule, 3 = same block on the Y axis.\n")
    w("pub const SKIP_RENDERING: &[(&str, u8)] = &[\n")
    for k in sorted(skip):
        w(f'    ("minecraft:{k}", {skip[k]}),\n')
    w("];\n\n")
    f32_table("FRICTION", "`Properties.friction` (default 0.6).", friction)
    f32_table("SPEED_FACTOR", "`Properties.speedFactor` (default 1.0).", speed)
    f32_table("JUMP_FACTOR", "`Properties.jumpFactor` (default 1.0).", jump)
    f32_table("BOUNCE", "`Properties.bounceRestitution` (default 0.0).", bounce)
    w("/// `makeStuckInBlock` multiplier from the block's `entityInside`.\n")
    w("pub const STUCK: &[(&str, [f64; 3])] = &[\n")
    for k in sorted(stuck):
        x, y, z = stuck[k]
        # A Java `0.9F` widened to double is the f32 value, not 0.9.
        vals = [f"{java_float(v)}f32 as f64" if v.endswith("F") else java_float(v) for v in (x, y, z)]
        w(f'    ("minecraft:{k}", [{", ".join(vals)}]),\n')
    w("];\n\n")
    str_list("TRAPDOORS", "Blocks whose class extends `TrapDoorBlock`.", trapdoors)
    str_list("LADDERS", "Blocks whose class extends `LadderBlock`.", ladders)
    str_list("FENCE_GATES", "Blocks whose class extends `FenceGateBlock`.", gates)
    for t, items in tags.items():
        str_list(t.upper() + "_TAG", f"`#minecraft:{t}`, expanded.", items)
    out.close()
    print(
        f"[gen_block_props] wrote {dest}: {len(skip)} skip, {len(friction)} friction, "
        f"{len(speed)} speed, {len(jump)} jump, {len(stuck)} stuck, "
        f"{len(trapdoors)} trapdoors, tags {[len(v) for v in tags.values()]}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
