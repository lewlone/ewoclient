#!/usr/bin/env python3
"""Extract per-state smooth-lighting facts from the decompiled Minecraft jar.

The mesher's smooth lighting (`rewo-mesh/src/smooth_light.rs`, transcribed
from vanilla's `client/renderer/block/BlockModelLighter`) reads four per-state
predicates that live in *code*, not in any datagen report:

  `BlockState.isViewBlocking(level, pos)`          the view-blocking predicate
  `BlockState.getShadeBrightness(...) == 0.2F`     dark (0.2) vs bright (1.0)
  `BlockState.emissiveRendering()`                 render at full bright
  the block model's `useAmbientOcclusion()`        flat vs ambient shading

Ground truth is `net/minecraft/world/level/block/Blocks.java` (builder calls
in the registrations) plus `net/minecraft/world/level/block/state/
BlockBehaviour.java` and the block classes:

* `isViewBlocking`. `BlockBehaviour.Properties` INITIALISES
  `isViewBlocking = this.isSuffocating`, i.e. the default suffocating lambda
  `state.blocksMotion() && state.isCollisionShapeFullBlock(level, pos)`.
  Field initialisers run at construction, so a later `.isSuffocating(x)`
  reassigns only `isSuffocating` — the default `isViewBlocking` keeps
  pointing at the DEFAULT lambda. Overriding `isSuffocating` therefore does
  NOT change `isViewBlocking`; only `.isViewBlocking(...)` does (a block that
  sets only `isSuffocating` is reported, since it is one source edit away
  from mattering). The default needs `blocksMotion()`, which is
  `!COBWEB && !BAMBOO_SAPLING && isSolid()`; `isSolid()` is
  `calculateSolid()` = forceSolidOn, else !forceSolidOff, else a shape test a
  full collision shape always passes — so those builder calls are extracted
  too, together with `dynamicShape()` (no shape cache -> never solid).
  `Properties.ofFullCopy` copies `isViewBlocking`/`isSuffocating`;
  `ofLegacyCopy` does not.

* `getShadeBrightness`. The default is
  `isCollisionShapeFullBlock ? 0.2F : 1.0F` (the bake's `ao_occluder`), and
  seven block classes override it (mud/soul sand dark unconditionally, the
  glass/void families bright, snow dark at 8 layers). Subclasses inherit, so
  the class hierarchy is walked — `StainedGlassBlock extends TransparentBlock`.

* `emissiveRendering`. `Properties.emissiveRendering`, default
  `var0 -> false`. Two call sites in `Blocks.java` (magma's constant true and
  the sculk-sensor phase predicate). `ofLegacyCopy` DOES copy this one, so
  `CALIBRATED_SCULK_SENSOR` shares SCULK_SENSOR's predicate object.

* model `useAmbientOcclusion()`. Not in `Blocks.java` at all: vanilla's
  `ModelBlockRenderer.tesselateBlock` reads
  `this.parts.getFirst().useAmbientOcclusion()`, and
  `ResolvedModel.findTopAmbientOcclusion` walks the model's parent chain for
  the first non-null `"ambientocclusion"` (default true). The asset bake
  computes that flag per model; this script only counts the models that set
  it. `crates/rewo-data/src/assets.rs` keys it off the state's FIRST model
  ref.

The registration walk is shared with `gen_block_light.py` (imported below) —
registration fields, `BlockBehaviour.Properties` helper factories inlined,
`ColorCollection` / `WeatheringCopperCollection` families resolved through
`BlockItemIds.java`. Predicate arguments are CLASSIFIED rather than matched by
name: a named `StatePredicate` constant is read from its own lambda body, so
`NOT_EXTENDED_PISTON` is understood as a property rule and
`NOT_CLOSED_SHULKER` as the one approximation this table makes (block-entity
state). A form the classifier does not understand is reported on stderr and
listed in the generated header — this script never silently defaults a form
it did not understand.

Run after a version bump:
    python tools/gen_ao_facts.py

Writes `crates/rewo-data/src/ao_facts_table.rs` directly (explicit UTF-8) —
a console redirect would re-encode the header in the local codepage and
produce a file rustc cannot read.
"""

import json
import os
import re
import sys
import textwrap

from gen_block_light import (
    BLOCKS,
    REGISTRY,
    ROOT,
    VERSION,
    balanced,
    iter_registrations,
    property_helpers,
    resolve,
    scan_single_ids,
)

BLOCK_BEHAVIOUR = os.path.join(
    ROOT, "net/minecraft/world/level/block/state/BlockBehaviour.java"
)

# A helper call that stands in for `getValue(...)`, named by SIGNATURE — a
# substring the expression must still contain — so a version bump that
# rewrites the expression stops matching and gets reported instead of silently
# keeping a stale rule. (signature, block-state property)
PREDICATE_CALLS = [
    # `SculkSensorBlock.getPhase(statex)` is `state.getValue(PHASE)`.
    ("SculkSensorBlock.getPhase", "sculk_sensor_phase"),
]


def expr_to_semicolon(text, start):
    """The expression at `start`, up to the `;` that ends it, paren-aware."""
    depth = 0
    i = start
    while i < len(text):
        c = text[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == ";" and depth == 0:
            return text[start:i]
        i += 1
    return text[start:]


def normalize_literal(lit):
    """A Java literal as `blocks.json` writes the property value.

    `8` -> `"8"`, `SculkSensorPhase.ACTIVE` -> `"active"`, booleans as-is.
    The emitted value is then CHECKED against the block's declared property
    values, so this mapping is verified rather than assumed.
    """
    if lit in ("true", "false"):
        return lit
    if re.fullmatch(r"\d+", lit):
        return lit
    if "." in lit:
        return lit.rsplit(".", 1)[1].lower()
    return lit.lower()


def classify_body(body):
    """Classify a predicate lambda body. `(kind, rule)`, or `(None, None)`.

    kind: "always", "never", "block_entity" (block-entity state, not
    block-state state) or "state" with rule `(property, operator, value)`
    meaning the predicate is true exactly when `property operator value`.
    """
    b = " ".join(body.split())
    if b == "true":
        return ("always", None)
    if b == "false":
        return ("never", None)
    if "getBlockEntity" in b:
        # `... instanceof ShulkerBoxBlockEntity be ? be.isClosed() : true`
        return ("block_entity", None)
    # `!statex.getValue(PistonBaseBlock.EXTENDED)` — a negated boolean
    # property is just `property == false`.
    m = re.fullmatch(r"!\s*\w+\.getValue\((?:\w+\.)?(\w+)\)", b)
    if m:
        return ("state", (m.group(1).lower(), "==", "false"))
    # `statex.getValue(SnowLayerBlock.LAYERS) >= 8` and friends. Only `==`
    # and `>=` are accepted; any other operator is reported, not guessed at.
    m = re.fullmatch(r"\w+\.getValue\((?:\w+\.)?(\w+)\)\s*(==|>=)\s*([\w.]+)", b)
    if m:
        return ("state", (m.group(1).lower(), m.group(2), normalize_literal(m.group(3))))
    # `SculkSensorBlock.getPhase(statex) == SculkSensorPhase.ACTIVE` — a
    # helper call standing in for getValue, resolved by signature.
    for sig, prop in PREDICATE_CALLS:
        m = re.fullmatch(re.escape(sig) + r"\(\s*\w+\s*\)\s*==\s*([\w.]+)", b)
        if m:
            return ("state", (prop, "==", normalize_literal(m.group(1))))
    return (None, None)


def classify_predicate(arg, constants, depth=0):
    """Classify an `isViewBlocking` / `emissiveRendering` argument.

    A bare UPPER_CASE name is a `BlockBehaviour.StatePredicate` constant from
    `Blocks.java` and is classified by reading its lambda body, not by
    matching its name.
    """
    a = " ".join(arg.split())
    if a == "Blocks::always":
        return ("always", None)
    if a == "Blocks::never":
        return ("never", None)
    if depth < 4 and re.fullmatch(r"[A-Z][A-Z0-9_]*", a) and a in constants:
        return classify_predicate(constants[a], constants, depth + 1)
    if "->" in a:
        return classify_body(a.split("->", 1)[1])
    return (None, None)


def classify_shade(body):
    """A `getShadeBrightness` override body: 1 = 0.2F, 2 = 1.0F, or a state
    rule meaning "0.2F exactly when the comparison holds". None = unparsed.
    """
    b = " ".join(body.split())
    if b == "return 0.2F;":
        return 1
    if b == "return 1.0F;":
        return 2
    # `SnowLayerBlock`: `return state.getValue(LAYERS) == 8 ? 0.2F : 1.0F;`
    m = re.fullmatch(
        r"return \w+\.getValue\((?:\w+\.)?(\w+)\)\s*==\s*([\w.]+)\s*\?\s*0\.2F\s*:\s*1\.0F;",
        b,
    )
    if m:
        return (m.group(1).lower(), "==", normalize_literal(m.group(2)))
    return None


def scan_predicate_constants(src):
    """`BlockBehaviour.StatePredicate NAME = lambda;` constants in Blocks.java.

    `NOT_CLOSED_SHULKER` and `NOT_EXTENDED_PISTON` — referenced bare by the
    registrations, so their bodies are what the classifier must read.
    """
    out = {}
    for m in re.finditer(r"static\s+final\s+BlockBehaviour\.StatePredicate\s+(\w+)\s*=", src):
        out[m.group(1)] = expr_to_semicolon(src, m.end())
    return out


def scan_shade_classes():
    """class -> (superclass, shade value) for `getShadeBrightness` overrides.

    Every class under `world/level/block` gets an entry so the walk can
    traverse to the override it inherits; a class without one is a `(super,
    None)` hop. `BlockBehaviour`'s default (the shape rule) is the walk's
    fall-through.
    """
    out = {}
    root = os.path.join(ROOT, "net/minecraft/world/level/block")
    for dirpath, _dirs, files in os.walk(root):
        for fn in files:
            if not fn.endswith(".java"):
                continue
            text = open(os.path.join(dirpath, fn), encoding="utf8", errors="replace").read()
            m = re.search(r"class\s+(\w+)(?:<[^>]*>)?\s+extends\s+(\w+)", text)
            if not m:
                continue
            cls, sup = m.group(1), m.group(2)
            value = None
            gm = re.search(r"float getShadeBrightness\([^)]*\)\s*\{(.*?)\n   \}", text, re.S)
            if gm and "isCollisionShapeFullBlock" not in gm.group(1):
                # The default body (`isCollisionShapeFullBlock ? 0.2F : 1.0F`)
                # is the fall-through the tables omit, not an override — it
                # lives on `BlockStateBase`, which this walk also sees.
                value = classify_shade(gm.group(1))
                if value is None:
                    print(f"  unparsed getShadeBrightness in {cls}", file=sys.stderr)
            out[cls] = (sup, value)
    return out


def scan_blocks_motion_excluded(single_ids):
    """Blocks `BlockStateBase.blocksMotion()` excludes by name.

    `return block != Blocks.COBWEB && block != Blocks.BAMBOO_SAPLING &&
    this.isSolid();`
    """
    text = open(BLOCK_BEHAVIOUR, encoding="utf8", errors="replace").read()
    m = re.search(r"boolean blocksMotion\(\)\s*\{(.*?)\n   \}", text, re.S)
    if not m:
        print("  unparsed blocksMotion()", file=sys.stderr)
        return []
    return [single_ids.get(f, f.lower()) for f in re.findall(r"!=\s*Blocks\.(\w+)", m.group(1))]


def copy_sources(parts):
    """`(field, kind)` for `ofFullCopy(FIELD)` / `ofLegacyCopy(FIELD)` calls."""
    out = []
    for part in parts:
        for m in re.finditer(r"of(Full|Legacy)Copy\((\w+)\)", part):
            out.append((m.group(2), m.group(1)))
    return out


def call_arg(parts, name):
    """The argument of the effective `.name(...)` call, or None.

    `parts[0]` is the registration's own builder chain and `parts[1:]` the
    inlined `Properties` helper bodies. The helper builds the base object and
    the chain runs after it, so a call in `parts[0]` wins even though helper
    bodies come later in the list; within one part the last call wins.
    """
    for part in parts:
        found = None
        for m in re.finditer(r"\." + name + r"\(", part):
            arg = balanced(part, m.end() - 1)
            found = arg[1:-1] if arg.endswith(")") else arg
        if found is not None:
            return found
    return None


def effective_arg(parts, by_field, name, full_only, depth=0):
    """The winning `.name(...)` argument, following property copies.

    `ofFullCopy` copies `isViewBlocking`/`isSuffocating`; `ofLegacyCopy`
    copies `emissiveRendering` but NOT those two — so CALIBRATED_SCULK_SENSOR
    shares SCULK_SENSOR's `emissiveRendering` predicate object, while
    TINTED_GLASS's explicit `.isViewBlocking(Blocks::never)` sits on top of
    whatever it copied from GLASS (and wins: the copy call runs first).
    """
    arg = call_arg(parts, name)
    if arg is not None:
        return arg
    if depth > 8:
        return None
    for field, kind in copy_sources(parts):
        if full_only and kind != "Full":
            continue
        src = by_field.get(field)
        if src:
            arg = effective_arg(src, by_field, name, full_only, depth + 1)
            if arg is not None:
                return arg
    return None


def with_copy_sources(parts, by_field, depth=0):
    """`parts` plus the property copies' parts — the flags
    (`forceSolidOn`/`forceSolidOff`/`dynamicShape`) are positive-only builder
    calls that BOTH copy kinds carry over, so presence anywhere wins.
    """
    out = list(parts)
    if depth > 8:
        return out
    for field, _kind in copy_sources(parts):
        src = by_field.get(field)
        if src:
            out.extend(with_copy_sources(src, by_field, depth + 1))
    return out


def main():
    src = open(BLOCKS, encoding="utf8", errors="replace").read()
    helpers = property_helpers(src)
    constants = scan_predicate_constants(src)
    shade_classes = scan_shade_classes()
    single_ids = scan_single_ids()
    registry = json.load(open(REGISTRY, encoding="utf8"))
    known = set(registry.keys())

    entries = list(iter_registrations(src, helpers))
    by_field = {}
    for field, _name, parts, _cls, _state_idx in entries:
        by_field.setdefault(field, parts)

    view_const = {}     # name -> 1 always, 2 never, 3 block-entity (approx)
    view_state = {}     # name -> (property, op, value)
    emissive = []       # constant-true names
    emissive_state = {}
    shade_const = {}    # name -> 1 dark (0.2F), 2 bright (1.0F)
    shade_state = {}
    force_on, force_off, dynamic_shape = [], [], []
    suffocating_only = []
    unparsed = []
    missing = []

    for field, name, parts, cls, _state_idx in entries:
        if f"minecraft:{name}" not in known:
            missing.append(name)
            continue

        # -- isViewBlocking --------------------------------------------------
        arg = effective_arg(parts, by_field, "isViewBlocking", full_only=True)
        if arg is not None:
            kind, rule = classify_predicate(arg, constants)
            if kind == "always":
                view_const[name] = 1
            elif kind == "never":
                view_const[name] = 2
            elif kind == "block_entity":
                view_const[name] = 3
            elif kind == "state":
                view_state[name] = rule
            else:
                unparsed.append(f"{name}: isViewBlocking({arg})")
        elif effective_arg(parts, by_field, "isSuffocating", full_only=True) is not None:
            # `isViewBlocking` captured the DEFAULT `isSuffocating` lambda at
            # construction, so this does not change it — but a block that
            # overrides only `isSuffocating` is one source edit away from
            # mattering, and the distinction is not worth silently dropping.
            suffocating_only.append(name)

        # -- emissiveRendering ----------------------------------------------
        arg = effective_arg(parts, by_field, "emissiveRendering", full_only=False)
        if arg is not None:
            kind, rule = classify_predicate(arg, constants)
            if kind == "always":
                emissive.append(name)
            elif kind == "state":
                emissive_state[name] = rule
            else:
                unparsed.append(f"{name}: emissiveRendering({arg})")

        # -- calculateSolid inputs ------------------------------------------
        flags = with_copy_sources(parts, by_field)
        if any(re.search(r"\.forceSolidOn\(\)", p) for p in flags):
            force_on.append(name)
        if any(re.search(r"\.forceSolidOff\(\)", p) for p in flags):
            force_off.append(name)
        if any(re.search(r"\.dynamicShape\(\)", p) for p in flags):
            dynamic_shape.append(name)

        # -- getShadeBrightness ---------------------------------------------
        if cls:
            value = resolve(shade_classes, cls, 1)
            if isinstance(value, tuple):
                shade_state[name] = value
            elif value is not None:
                shade_const[name] = value

    # -- check every property rule against the block's declared values -------
    def check_rule(name, rule):
        prop, _op, value = rule
        values = registry.get(f"minecraft:{name}", {}).get("properties", {}).get(prop)
        if values is None:
            unparsed.append(f"{name}: no `{prop}` property in blocks.json")
            return False
        if value not in values:
            unparsed.append(f"{name}: `{prop}` has no value {value} (has {values})")
            return False
        return True

    view_state = {n: r for n, r in view_state.items() if check_rule(n, r)}
    emissive_state = {n: r for n, r in emissive_state.items() if check_rule(n, r)}
    shade_state = {n: r for n, r in shade_state.items() if check_rule(n, r)}
    motion_excluded = scan_blocks_motion_excluded(single_ids)

    if missing:
        print(
            f"  {len(missing)} generated names are not in the registry "
            f"(first: {missing[:3]}) — the naming rule drifted",
            file=sys.stderr,
        )
    for name in suffocating_only:
        print(f"  {name}: isSuffocating overridden but isViewBlocking is not", file=sys.stderr)

    # -- emit ---------------------------------------------------------------
    dest = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "crates", "rewo-data", "src", "ao_facts_table.rs",
    )
    out = open(dest, "w", encoding="utf8", newline="\n")
    out.write("//! GENERATED by `tools/gen_ao_facts.py` — do not edit.\n")
    out.write(f"//!\n//! Source: decompiled {VERSION} `Blocks.java` + `BlockBehaviour.java`\n")
    out.write("//! + the block classes' `getShadeBrightness` overrides.\n")
    out.write("//! Re-run after a version bump; see the script header for the\n")
    out.write("//! extraction rules and the one approximation this table makes.\n//!\n")
    always = sum(1 for v in view_const.values() if v == 1)
    never = sum(1 for v in view_const.values() if v == 2)
    be = sum(1 for v in view_const.values() if v == 3)
    dark = sum(1 for v in shade_const.values() if v == 1)
    bright = sum(1 for v in shade_const.values() if v == 2)
    counts = (
        f"{len(view_const)} constant view-blocking ({always} always, {never} never,"
        f" {be} block-entity), {len(view_state)} property-driven,"
        f" {len(emissive)} constant emissive, {len(emissive_state)} property-driven,"
        f" {len(shade_const)} shade ({dark} dark, {bright} bright),"
        f" {len(shade_state)} property-driven, {len(force_on)} forceSolidOn,"
        f" {len(force_off)} forceSolidOff, {len(dynamic_shape)} dynamicShape,"
        f" {len(motion_excluded)} blocksMotion name exclusions."
    )
    for line in textwrap.wrap(counts, 68):
        out.write(f"//! {line}\n")
    approx = []
    if be:
        approx.append(
            f"{be} shulker boxes: `NOT_CLOSED_SHULKER` is\n"
            "//!     `level.getBlockEntity(pos) instanceof ShulkerBoxBlockEntity be\n"
            "//!     ? be.isClosed() : true` — block-entity state. Baked as always\n"
            "//!     true (closed): a placed shulker box is closed except while a\n"
            "//!     player holds it open."
        )
    for u in sorted(unparsed):
        approx.append(u.replace("\n", " "))
    if suffocating_only:
        approx.append(
            "isSuffocating overridden without isViewBlocking: "
            + ", ".join(sorted(suffocating_only))
            + "\n//!     (harmless today: `isViewBlocking` captured the DEFAULT suffocating\n"
            "//!     lambda, so the override does not reach it)"
        )
    if approx:
        out.write("//!\n//! Approximated:\n")
        for a in approx:
            out.write(f"//!   - {a}\n")
    out.write("\n")

    out.write("/// Blocks whose `isViewBlocking` predicate is a constant.\n")
    out.write("///\n")
    out.write("/// Values: 1 = `Blocks::always`, 2 = `Blocks::never`, 3 =\n")
    out.write("/// `NOT_CLOSED_SHULKER` (block-entity state — approximated as true, see\n")
    out.write("/// the header). A block in neither this table nor `VIEW_BLOCKING_STATE`\n")
    out.write("/// keeps the default `BlockBehaviour.Properties` predicate (the one\n")
    out.write("/// `isSuffocating` starts with):\n")
    out.write("/// `state.blocksMotion() && state.isCollisionShapeFullBlock(level, pos)`.\n")
    out.write("pub const VIEW_BLOCKING: &[(&str, u8)] = &[\n")
    for k in sorted(view_const):
        out.write(f'    ("minecraft:{k}", {view_const[k]}),\n')
    out.write("];\n\n")

    out.write("/// Property-driven `isViewBlocking` predicates.\n")
    out.write("///\n")
    out.write("/// `(block, property, operator, value)` — the predicate is true exactly\n")
    out.write("/// when `property operator value` holds on the state. Operator is `==`\n")
    out.write("/// (string equality, `blocks.json` form) or `>=` (both sides numeric).\n")
    out.write("/// `NOT_EXTENDED_PISTON` (`!getValue(EXTENDED)`) lands here as\n")
    out.write("/// `extended == false`; SNOW's `layers >= 8` lambda as-is.\n")
    out.write("pub const VIEW_BLOCKING_STATE: &[(&str, &str, &str, &str)] = &[\n")
    for k in sorted(view_state):
        p, op, v = view_state[k]
        out.write(f'    ("minecraft:{k}", "{p}", "{op}", "{v}"),\n')
    out.write("];\n\n")

    out.write("/// Blocks whose `BlockState.emissiveRendering()` is always true\n")
    out.write("/// (`Blocks.java`'s `.emissiveRendering(var0 -> true)`; magma).\n")
    out.write("pub const EMISSIVE_ALWAYS: &[&str] = &[\n")
    for k in sorted(emissive):
        out.write(f'    "minecraft:{k}",\n')
    out.write("];\n\n")

    out.write("/// Property-driven `emissiveRendering`, same shape as\n")
    out.write("/// `VIEW_BLOCKING_STATE`. The sculk sensors: the phase predicate, which\n")
    out.write("/// `ofLegacyCopy` carries over to `calibrated_sculk_sensor` unchanged.\n")
    out.write("pub const EMISSIVE_STATE: &[(&str, &str, &str, &str)] = &[\n")
    for k in sorted(emissive_state):
        p, op, v = emissive_state[k]
        out.write(f'    ("minecraft:{k}", "{p}", "{op}", "{v}"),\n')
    out.write("];\n\n")

    out.write("/// Class overrides of `BlockBehaviour.getShadeBrightness`, resolved to\n")
    out.write("/// block names through the class hierarchy (`StainedGlassBlock extends\n")
    out.write("/// TransparentBlock` inherits its 1.0F). Values: 1 = 0.2F (shade dark),\n")
    out.write("/// 2 = 1.0F. A block in neither this table nor `SHADE_STATE` uses the\n")
    out.write("/// default: `isCollisionShapeFullBlock ? 0.2F : 1.0F`.\n")
    out.write("pub const SHADE: &[(&str, u8)] = &[\n")
    for k in sorted(shade_const):
        out.write(f'    ("minecraft:{k}", {shade_const[k]}),\n')
    out.write("];\n\n")

    out.write("/// Property-driven `getShadeBrightness`, same shape as\n")
    out.write("/// `VIEW_BLOCKING_STATE` but meaning 0.2F (dark) exactly when the\n")
    out.write("/// comparison holds (snow's `LAYERS == 8 ? 0.2F : 1.0F`).\n")
    out.write("pub const SHADE_STATE: &[(&str, &str, &str, &str)] = &[\n")
    for k in sorted(shade_state):
        p, op, v = shade_state[k]
        out.write(f'    ("minecraft:{k}", "{p}", "{op}", "{v}"),\n')
    out.write("];\n\n")

    out.write("/// `BlockBehaviour.Properties.forceSolidOn()` callers —\n")
    out.write("/// `BlockStateBase.calculateSolid()` answers true whatever the shape.\n")
    out.write("pub const FORCE_SOLID_ON: &[&str] = &[\n")
    for k in sorted(force_on):
        out.write(f'    "minecraft:{k}",\n')
    out.write("];\n\n")

    out.write("/// `BlockBehaviour.Properties.forceSolidOff()` callers —\n")
    out.write("/// `calculateSolid()` answers false whatever the shape.\n")
    out.write("pub const FORCE_SOLID_OFF: &[&str] = &[\n")
    for k in sorted(force_off):
        out.write(f'    "minecraft:{k}",\n')
    out.write("];\n\n")

    out.write("/// `BlockBehaviour.Properties.dynamicShape()` callers — no shape cache,\n")
    out.write("/// so `calculateSolid()` is false unless `FORCE_SOLID_ON` also applies.\n")
    out.write("pub const DYNAMIC_SHAPE: &[&str] = &[\n")
    for k in sorted(dynamic_shape):
        out.write(f'    "minecraft:{k}",\n')
    out.write("];\n\n")

    out.write("/// `BlockStateBase.blocksMotion()` never answers true for these, by name:\n")
    out.write("/// `block != Blocks.COBWEB && block != Blocks.BAMBOO_SAPLING && isSolid()`.\n")
    out.write("pub const BLOCKS_MOTION_EXCLUDED: &[&str] = &[\n")
    for k in sorted(motion_excluded):
        out.write(f'    "minecraft:{k}",\n')
    out.write("];\n")
    out.close()

    print(f"[gen_ao_facts] wrote {dest}", file=sys.stderr)
    print(
        f"[gen_ao_facts] {len(view_const)} view-const "
        f"({always} always, {never} never, {be} be), "
        f"{len(view_state)} view-state, {len(emissive)} emissive, "
        f"{len(emissive_state)} emissive-state, {dark} shade-dark, "
        f"{bright} shade-bright, {len(shade_state)} shade-state, "
        f"{len(force_on)}/{len(force_off)}/{len(dynamic_shape)} solid flags, "
        f"{len(motion_excluded)} motion-excluded, "
        f"{len(unparsed)} approximated, {len(missing)} unknown-name",
        file=sys.stderr,
    )
    for u in unparsed:
        print(f"  approx: {u}", file=sys.stderr)
    for name in suffocating_only:
        print(f"  suffocating-only: {name}", file=sys.stderr)


if __name__ == "__main__":
    main()
