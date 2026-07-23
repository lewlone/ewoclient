#!/usr/bin/env python3
"""Extract per-block light data from the decompiled Minecraft jar.

Rewo's light engine needs facts that live in *code*, not in any datagen report:
a block's light EMISSION, whether it can occlude (`noOcclusion()`), and two
virtual overrides that change light behaviour. Everything else in vanilla's
rule is derivable from what the asset bake already produces (full-cube shape +
fluid-ness) — see `rewo-world/src/light.rs` for the rule this feeds.

Ground truth is `net/minecraft/world/level/block/Blocks.java`, where most
blocks are registered one field at a time:

    public static final Block TORCH = register(
        BlockItemIds.TORCH, p -> new TorchBlock(...),
        BlockBehaviour.Properties.of().noCollision().instabreak().lightLevel(s -> 14)...);

The registry name is the SCREAMING_SNAKE field name lowercased. Properties can
also arrive via helper factories declared in the same file (`leavesProperties`,
etc.), so helper bodies are inlined before matching.

Two facts are *virtual method overrides* on the block class rather than builder
calls, and both change light: `propagatesSkylightDown` (glass returns true, so
sky passes it at full strength) and `getLightDampening` (leaves pin it to 1).
The registration lambda names the implementation class, so those are resolved
by walking the class hierarchy — `StainedGlassBlock extends TransparentBlock`
inherits the glass behaviour.

Families are *not* declared one field per block. Dye and copper variants go
through `ColorCollection` / `WeatheringCopperCollection`, whose registry names
come from id tables in `net/minecraft/references/BlockItemIds.java`:

    DYED_SHULKER_BOX = createSimpleColored("shulker_box")   -> <colour>_shulker_box
    COPPER_BULB      = createSimpleCopper("copper_bulb")    -> copper_bulb,
                       exposed_copper_bulb, ..., waxed_oxidized_copper_bulb
    COPPER_BLOCK     = prefixWithState(same(ByState(         <- the one special
                           "copper_block", "copper", "copper", "copper")))
                                                            -> copper_block,
                       exposed_copper, weathered_copper, oxidized_copper, ...

Those tables are parsed rather than guessed at, because the copper naming is
irregular (`copper_block` but `exposed_copper`) and because copper emission
varies with the weathering state — a copper bulb is 15/12/8/4 as it oxidises.

Every generated name is checked against the block registry, so the naming rules
stay verified rather than assumed.

Run after a version bump:
    python tools/gen_block_light.py

Writes `crates/rewo-data/src/block_light.rs` directly (explicit UTF-8) rather
than to stdout — a console redirect would re-encode the header in the local
codepage and produce a file rustc cannot read.

Anything unparsed is reported on stderr and listed in the generated header —
this script never silently defaults a form it did not understand.
"""

import json
import os
import re
import sys

VERSION = os.environ.get("REWO_VERSION", "26.2")
ROOT = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", VERSION, "decompiled")
BLOCKS = os.path.join(ROOT, "net/minecraft/world/level/block/Blocks.java")
IDS = os.path.join(ROOT, "net/minecraft/references/BlockItemIds.java")
BLOCK_IDS = os.path.join(ROOT, "net/minecraft/references/BlockIds.java")
REGISTRY = os.path.join(ROOT, "..", "datagen", "generated", "reports", "blocks.json")

# ColorCollection.NAMES, in declaration order.
DYE_COLORS = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink",
    "gray", "light_gray", "cyan", "purple", "blue", "brown", "green",
    "red", "black",
]

# WeatheringCopperCollection.PREFIXES, flattened weathering-then-waxed, each
# paired with the weather-state index it carries (0 = unaffected … 3 =
# oxidized). The state matters because emission varies with it.
COPPER_PREFIXES = [
    ("", 0), ("exposed_", 1), ("weathered_", 2), ("oxidized_", 3),
    ("waxed_", 0), ("waxed_exposed_", 1),
    ("waxed_weathered_", 2), ("waxed_oxidized_", 3),
]
WEATHER_STATES = ["UNAFFECTED", "EXPOSED", "WEATHERED", "OXIDIZED"]

# How a `propagatesSkylightDown` override body maps to a per-state value.
CONST_TRUE, CONST_FALSE, NOT_WATERLOGGED = 1, 0, 2


def balanced(text, start):
    """Return the substring from `start` (index of '(') through its match."""
    depth = 0
    for i in range(start, len(text)):
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    return text[start:]


def classify_bool(body):
    """Classify a `propagatesSkylightDown` body. None = not understood."""
    b = " ".join(body.split())
    if b == "return true;":
        return CONST_TRUE
    if b == "return false;":
        return CONST_FALSE
    # `getFluidState().isEmpty()` and `!getValue(WATERLOGGED)` are the same
    # predicate: the state is dry.
    if "getFluidState().isEmpty()" in b or "getValue(WATERLOGGED)" in b:
        return NOT_WATERLOGGED
    return None


def scan_block_classes():
    """class name -> (superclass, propagates code, dampening value)."""
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
            prop = damp = None
            pm = re.search(
                r"protected boolean propagatesSkylightDown\([^)]*\)\s*\{(.*?)\n   \}",
                text, re.S)
            if pm:
                prop = classify_bool(pm.group(1))
                if prop is None:
                    print(f"  unparsed propagatesSkylightDown in {cls}", file=sys.stderr)
            dm = re.search(
                r"protected int getLightDampening\([^)]*\)\s*\{(.*?)\n   \}",
                text, re.S)
            if dm:
                d = " ".join(dm.group(1).split())
                cm = re.match(r"return (\d+);", d)
                if cm:
                    damp = int(cm.group(1))
                else:
                    print(f"  unparsed getLightDampening in {cls}", file=sys.stderr)
            out[cls] = (sup, prop, damp)
    return out


def resolve(classes, cls, idx):
    """First value for slot `idx` found walking up from `cls`."""
    seen = set()
    while cls and cls not in seen:
        seen.add(cls)
        entry = classes.get(cls)
        if not entry:
            return None
        if entry[idx] is not None:
            return entry[idx]
        cls = entry[0]
    return None


def scan_single_ids():
    """`BlockIds`/`BlockItemIds` field -> registry name, for single blocks.

    The `Blocks.java` field name is *usually* the registry name, but not
    always: `POTTED_AZALEA = register(BlockIds.POTTED_AZALEA_BUSH, ...)`.
    Resolving through the id reference removes the assumption entirely.
    """
    out = {}
    for path in (BLOCK_IDS, IDS):
        try:
            text = open(path, encoding="utf8", errors="replace").read()
        except OSError:
            continue
        for m in re.finditer(
            r"public static final \w+(?:<\w+>)? ([A-Z0-9_]+) = \w*[Cc]reate\(\"([a-z0-9_/]+)\"\)",
            text,
        ):
            out[m.group(1)] = m.group(2)
    return out


def scan_id_tables():
    """`BlockItemIds` field -> [(registry name, weather-state index or None)].

    Only the family tables matter here; a single block takes its name from the
    `Blocks.java` field directly.
    """
    src = open(IDS, encoding="utf8", errors="replace").read()
    out = {}

    for m in re.finditer(
        r"ColorCollection<BlockItemId>\s+(\w+)\s*=\s*createSimpleColored\(\"(\w+)\"\)", src
    ):
        field, base = m.group(1), m.group(2)
        out[field] = [(f"{c}_{base}", None) for c in DYE_COLORS]

    for m in re.finditer(
        r"WeatheringCopperCollection<BlockItemId>\s+(\w+)\s*=\s*createSimpleCopper\(\"(\w+)\"\)", src
    ):
        field, base = m.group(1), m.group(2)
        out[field] = [(prefix + base, st) for prefix, st in COPPER_PREFIXES]

    # The one hand-written copper table: `prefixWithState(same(ByState(a,b,c,d)))`
    # where the four ids differ — `copper_block` but `exposed_copper`.
    for m in re.finditer(
        r"ByState<String>\s+(\w+)\s*=\s*new WeatheringCopperCollection\.ByState<>\(\s*"
        r"\"(\w+)\",\s*\"(\w+)\",\s*\"(\w+)\",\s*\"(\w+)\"", src
    ):
        holder, by_state = m.group(1), list(m.group(2, 3, 4, 5))
        for u in re.finditer(
            r"WeatheringCopperCollection<BlockItemId>\s+(\w+)\s*=\s*"
            r"WeatheringCopperCollection\.prefixWithState\(\s*"
            r"WeatheringCopperCollection\.same\(" + holder + r"\)", src
        ):
            out[u.group(1)] = [(prefix + by_state[st], st) for prefix, st in COPPER_PREFIXES]
    return out


def parse_emission(text, state_idx=None):
    """`(constant, lit_conditional)` emission from a properties expression.

    Handles `lightLevel(s -> N)`, `lightLevel(litBlockEmission(N))`, and the
    per-weather-state `litBlockEmission(switch (p) { case EXPOSED -> 12; … })`
    the copper bulb uses. Returns `(None, None)` when there is no lightLevel at
    all, and `("?", None)` when the form was not understood.
    """
    if "lightLevel(" not in text:
        return (None, None)
    if state_idx is not None:
        sw = re.search(r"litBlockEmission\(\s*switch\s*\([^)]*\)\s*\{(.*?)\}", text, re.S)
        if sw:
            arms = dict(
                (a, int(b)) for a, b in re.findall(r"case (\w+)\s*->\s*(\d+)", sw.group(1))
            )
            v = arms.get(WEATHER_STATES[state_idx])
            if v is not None:
                return (None, v)
    c = re.search(r"lightLevel\(\s*\w+\s*->\s*(\d+)\s*\)", text)
    if c:
        return (int(c.group(1)), None)
    lit = re.search(r"litBlockEmission\(\s*(\d+)\s*\)", text)
    if lit:
        return (None, int(lit.group(1)))
    return ("?", None)


def impl_class(body):
    """The implementation class named by a registration lambda, if any."""
    m = re.search(r"new\s+([A-Z]\w*Block)\s*[(<]", body) or re.search(
        r"([A-Z]\w*Block)::new", body)
    return m.group(1) if m else None


def main():
    src = open(BLOCKS, encoding="utf8", errors="replace").read()
    classes = scan_block_classes()
    id_tables = scan_id_tables()
    single_ids = scan_single_ids()
    known = set(json.load(open(REGISTRY, encoding="utf8")).keys())

    # -- helper factories that return Properties ----------------------------
    # e.g. `private static BlockBehaviour.Properties leavesProperties(...) { … }`
    helpers = {}
    for m in re.finditer(
        r"(?:private|public)\s+static\s+BlockBehaviour\.Properties\s+(\w+)\s*\(", src
    ):
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

    emission = {}       # name -> constant emission
    lit_emission = {}   # name -> emission while the `lit` property is true
    no_occlude = []     # names that called noOcclusion()
    propagate = {}      # name -> CONST_TRUE / CONST_FALSE / NOT_WATERLOGGED
    damp_over = {}      # name -> forced dampening
    unparsed = []
    missing = []

    def record(name, expanded, cls, state_idx=None):
        """Record every light fact for one registry name."""
        if f"minecraft:{name}" not in known:
            missing.append(name)
            return
        if "noOcclusion()" in expanded:
            no_occlude.append(name)
        const, lit = parse_emission(expanded, state_idx)
        if const == "?":
            # State-dependent forms we do not model exactly (cave vines,
            # vaults, …). Take the max literal so the block still lights.
            inner = balanced(expanded, expanded.index("lightLevel(") + len("lightLevel"))
            lits = [v for v in (int(x) for x in re.findall(r"\b(\d{1,2})\b", inner))
                    if 1 <= v <= 15]
            if lits:
                emission[name] = max(lits)
                unparsed.append(f"{name}: state-dependent, used max={max(lits)}")
            else:
                unparsed.append(f"{name}: lightLevel() form not understood")
        elif const is not None:
            emission[name] = const
        elif lit is not None:
            lit_emission[name] = lit
        if cls:
            p = resolve(classes, cls, 1)
            if p is not None:
                propagate[name] = p
            d = resolve(classes, cls, 2)
            if d is not None:
                damp_over[name] = d

    # -- single-field registrations -----------------------------------------
    for m in re.finditer(
        r"public\s+static\s+final\s+Block\s+([A-Z0-9_]+)\s*=\s*\w+\s*\(", src, re.M
    ):
        body = balanced(src, m.end() - 1)
        expanded = body
        for hname, hbody in helpers.items():
            if hname + "(" in expanded:
                expanded += "\n" + hbody
        # Prefer the id reference over the field name — they differ for a
        # handful of blocks (POTTED_AZALEA -> potted_azalea_bush).
        ref = re.search(r"Block(?:Item)?Ids\.(\w+)", body)
        name = single_ids.get(ref.group(1)) if ref else None
        record(name or m.group(1).lower(), expanded, impl_class(body))

    # -- family registrations ------------------------------------------------
    # `ColorCollection<Block> STAINED_GLASS = ColorCollection.registerBlocks(
    #      BlockItemIds.STAINED_GLASS, …)` registers 16 (copper: 8) blocks whose
    # names come from the id table.
    for m in re.finditer(
        r"public\s+static\s+final\s+(?:Color|WeatheringCopper)Collection<Block>\s+"
        r"[A-Z0-9_]+\s*=\s*\w+\s*\.\s*registerBlocks\s*\(", src, re.M
    ):
        body = balanced(src, m.end() - 1)
        ref = re.search(r"BlockItemIds\.(\w+)", body)
        if not ref:
            continue
        names = id_tables.get(ref.group(1))
        if not names:
            print(f"  no id table for BlockItemIds.{ref.group(1)}", file=sys.stderr)
            continue
        cls = impl_class(body)
        for name, state_idx in names:
            record(name, body, cls, state_idx)

    if missing:
        print(
            f"  {len(missing)} generated names are not in the registry "
            f"(first: {missing[:3]}) — the naming rule drifted",
            file=sys.stderr,
        )

    # -- emit ----------------------------------------------------------------
    dest = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "crates", "rewo-data", "src", "block_light.rs",
    )
    out = open(dest, "w", encoding="utf8", newline="\n")
    out.write("//! GENERATED by `tools/gen_block_light.py` — do not edit.\n")
    out.write(f"//!\n//! Source: decompiled {VERSION} `Blocks.java` + `BlockItemIds.java`.\n")
    out.write("//! Re-run after a version bump; see the script header for the\n")
    out.write("//! extraction rules and why this cannot come from a datagen report.\n//!\n")
    out.write(f"//! {len(emission)} constant emitters, {len(lit_emission)} lit-conditional,\n")
    out.write(f"//! {len(no_occlude)} non-occluding, {len(propagate)} sky-propagation\n")
    out.write(f"//! overrides, {len(damp_over)} dampening overrides.\n")
    if unparsed:
        out.write("//!\n//! Approximated (state-dependent emission):\n")
        for u in sorted(unparsed):
            out.write(f"//!   - {u}\n")
    out.write("\n")

    out.write("/// Blocks whose light emission is the same in every state.\n")
    out.write("pub const EMISSION: &[(&str, u8)] = &[\n")
    for k in sorted(emission):
        out.write(f'    ("minecraft:{k}", {emission[k]}),\n')
    out.write("];\n\n")

    out.write("/// Blocks that emit only while their `lit` property is true.\n")
    out.write("pub const LIT_EMISSION: &[(&str, u8)] = &[\n")
    for k in sorted(lit_emission):
        out.write(f'    ("minecraft:{k}", {lit_emission[k]}),\n')
    out.write("];\n\n")

    out.write("/// How a block answers `propagatesSkylightDown`, when its class\n")
    out.write("/// overrides the default `!fullCube && noFluid`. Values:\n")
    out.write("/// 0 = never, 1 = always, 2 = only while not waterlogged.\n")
    out.write("pub const SKY_PROPAGATE: &[(&str, u8)] = &[\n")
    for k in sorted(propagate):
        out.write(f'    ("minecraft:{k}", {propagate[k]}),\n')
    out.write("];\n\n")

    out.write("/// Blocks whose class pins `getLightDampening` to a constant,\n")
    out.write("/// overriding the shape rule (leaves = 1, tinted glass = 15).\n")
    out.write("pub const DAMPENING_OVERRIDE: &[(&str, u8)] = &[\n")
    for k in sorted(damp_over):
        out.write(f'    ("minecraft:{k}", {damp_over[k]}),\n')
    out.write("];\n\n")

    out.write("/// Blocks that called `noOcclusion()` — full-cube shaped members\n")
    out.write("/// of this set dampen light by 1 instead of 15 (glass, leaves, …).\n")
    out.write("pub const NO_OCCLUDE: &[&str] = &[\n")
    for k in sorted(no_occlude):
        out.write(f'    "minecraft:{k}",\n')
    out.write("];\n")
    out.close()

    print(f"[gen_block_light] wrote {dest}", file=sys.stderr)
    print(
        f"[gen_block_light] {len(emission)} emitters, {len(lit_emission)} lit, "
        f"{len(no_occlude)} no-occlude, {len(propagate)} sky-overrides, "
        f"{len(damp_over)} damp-overrides, {len(unparsed)} approximated, "
        f"{len(missing)} unknown-name",
        file=sys.stderr,
    )
    for u in unparsed:
        print(f"  approx: {u}", file=sys.stderr)


if __name__ == "__main__":
    main()
