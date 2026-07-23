#!/usr/bin/env python3
"""Extract per-block light data from the decompiled Minecraft jar.

Rewo's light engine needs two facts that live in *code*, not in any datagen
report: a block's light EMISSION and whether it can occlude (`noOcclusion()`).
Everything else in vanilla's light rule is derivable from data the asset bake
already produces (full-cube shape + fluid-ness) — see `rewo-world/src/light.rs`
for the rule this feeds.

Ground truth is `net/minecraft/world/level/block/Blocks.java`, where every
block is registered as

    public static final Block TORCH = register(
        BlockItemIds.TORCH, p -> new TorchBlock(...),
        BlockBehaviour.Properties.of().noCollision().instabreak().lightLevel(s -> 14)...);

The registry name is the SCREAMING_SNAKE field name lowercased. Properties can
also arrive via helper factories declared in the same file (`leavesProperties`,
etc.), so helper bodies are inlined before matching.

Two further facts are *virtual method overrides* on the block class rather than
builder calls, and both change light: `propagatesSkylightDown` (glass returns
true, so sky passes it at full strength) and `getLightDampening` (leaves pin it
to 1). The registration lambda names the implementation class, so those are
resolved by walking the class hierarchy — `StainedGlassBlock extends
TransparentBlock` inherits the glass behaviour.

Run after a version bump:
    python tools/gen_block_light.py

Writes `crates/rewo-data/src/block_light.rs` directly (explicit UTF-8) rather
than to stdout — a console redirect would re-encode the header in the local
codepage and produce a file rustc cannot read.

Anything unparsed is reported on stderr and listed in the generated header —
this script never silently defaults a form it did not understand.
"""

import os
import re
import sys

VERSION = os.environ.get("REWO_VERSION", "26.2")
ROOT = os.path.join(
    os.environ["APPDATA"], "EwoClient", "rewo", VERSION, "decompiled"
)
BLOCKS = os.path.join(ROOT, "net/minecraft/world/level/block/Blocks.java")
REGISTRY = os.path.join(ROOT, "..", "datagen", "generated", "reports", "blocks.json")

# Dye families are registered in one `ColorCollection.registerBlocks` call
# rather than as 16 fields, so their names are expanded here. Every
# generated name is checked against the block registry, which turns the
# naming convention into something verified instead of assumed.
DYE_COLORS = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink",
    "gray", "light_gray", "cyan", "purple", "blue", "brown", "green",
    "red", "black",
]


def balanced(text, start):
    """Return the substring from `start` (index of '(') through its match."""
    depth = 0
    for i in range(start, len(text)):
        c = text[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    return text[start:]


# How an override body maps to a per-state value.
CONST_TRUE, CONST_FALSE, NOT_WATERLOGGED = 1, 0, 2


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
            text = open(os.path.join(dirpath, fn), encoding="utf8",
                        errors="replace").read()
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


def main():
    src = open(BLOCKS, encoding="utf8", errors="replace").read()
    classes = scan_block_classes()
    import json
    known = set(json.load(open(REGISTRY, encoding="utf8")).keys())

    # -- 1. helper factories that return Properties -------------------------
    # e.g. `private static BlockBehaviour.Properties leavesProperties(...) { ... }`
    helpers = {}
    for m in re.finditer(
        r"(?:private|public)\s+static\s+BlockBehaviour\.Properties\s+(\w+)\s*\(",
        src,
    ):
        name = m.group(1)
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
        helpers[name] = src[brace : j + 1]

    # -- 2. every registered block ------------------------------------------
    emission = {}       # name -> constant emission
    lit_emission = {}   # name -> emission when the `lit` property is true
    no_occlude = []     # names that called noOcclusion()
    propagate = {}      # name -> CONST_TRUE / CONST_FALSE / NOT_WATERLOGGED
    damp_over = {}      # name -> forced dampening
    unparsed = []

    decl = re.compile(
        r"public\s+static\s+final\s+Block\s+([A-Z0-9_]+)\s*=\s*(\w+)\s*\(", re.M
    )
    for m in decl.finditer(src):
        field, _fn = m.group(1), m.group(2)
        name = field.lower()
        body = balanced(src, m.end() - 1)

        # inline any helper-factory call so its properties are visible
        expanded = body
        for hname, hbody in helpers.items():
            if hname + "(" in expanded:
                expanded += "\n" + hbody

        if "noOcclusion()" in expanded:
            no_occlude.append(name)

        # Implementation class: `SomeBlock::new` or `p -> new SomeBlock(...)`.
        impl = re.search(r"new\s+([A-Z]\w*Block)\s*[(<]", body) or re.search(
            r"([A-Z]\w*Block)::new", body)
        if impl:
            cls = impl.group(1)
            p = resolve(classes, cls, 1)
            if p is not None:
                propagate[name] = p
            d = resolve(classes, cls, 2)
            if d is not None:
                damp_over[name] = d

        if "lightLevel(" in expanded:
            # lightLevel(state -> N)  — a plain constant
            c = re.search(r"lightLevel\(\s*\w+\s*->\s*(\d+)\s*\)", expanded)
            # lightLevel(litBlockEmission(N)) — N only when `lit` is true
            lit = re.search(r"litBlockEmission\(\s*(\d+)\s*\)", expanded)
            if c:
                emission[name] = int(c.group(1))
            elif lit:
                lit_emission[name] = int(lit.group(1))
            else:
                # state-dependent forms we do not model exactly (cave vines,
                # vaults, …). Record the max literal so the block still lights.
                lits = [int(x) for x in re.findall(r"\b(\d{1,2})\b",
                        re.search(r"lightLevel\(", expanded) and
                        balanced(expanded, expanded.index("lightLevel(") + len("lightLevel")) or "")]
                lits = [v for v in lits if 1 <= v <= 15]
                if lits:
                    emission[name] = max(lits)
                    unparsed.append(f"{name}: state-dependent, used max={max(lits)}")
                else:
                    unparsed.append(f"{name}: lightLevel() form not understood")

    # -- 2b. colour families -------------------------------------------------
    # `public static final ColorCollection<Block> STAINED_GLASS =
    #     ColorCollection.registerBlocks(..., StainedGlassBlock::new, ...)`
    # registers 16 blocks named `<colour>_stained_glass`. A leading `DYED_` is
    # a code-side disambiguator, not part of the registry name.
    missing = []
    for m in re.finditer(
        r"public\s+static\s+final\s+ColorCollection<Block>\s+([A-Z0-9_]+)\s*=\s*\w+\s*\.\s*\w+\s*\(",
        src,
    ):
        field = m.group(1)
        body = balanced(src, m.end() - 1)
        base = field.lower()
        if base.startswith("dyed_"):
            base = base[5:]
        impl = re.search(r"new\s+([A-Z]\w*Block)\s*[(<]", body) or re.search(
            r"([A-Z]\w*Block)::new", body)
        for colour in DYE_COLORS:
            name = f"{colour}_{base}"
            if f"minecraft:{name}" not in known:
                missing.append(name)
                continue
            if "noOcclusion()" in body:
                no_occlude.append(name)
            c = re.search(r"lightLevel\(\s*\w+\s*->\s*(\d+)\s*\)", body)
            if c:
                emission[name] = int(c.group(1))
            if impl:
                p = resolve(classes, impl.group(1), 1)
                if p is not None:
                    propagate[name] = p
                d = resolve(classes, impl.group(1), 2)
                if d is not None:
                    damp_over[name] = d
    if missing:
        print(
            f"  {len(missing)} expanded colour names not in the registry "
            f"(first: {missing[:3]}) — check DYE_COLORS / the naming rule",
            file=sys.stderr,
        )

    # -- 3. emit ------------------------------------------------------------
    dest = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "crates", "rewo-data", "src", "block_light.rs",
    )
    out = open(dest, "w", encoding="utf8", newline="\n")
    out.write("//! GENERATED by `tools/gen_block_light.py` — do not edit.\n")
    out.write(f"//!\n//! Source: decompiled {VERSION} `Blocks.java`.\n")
    out.write("//! Re-run after a version bump; see the script header for the\n")
    out.write("//! extraction rules and why this cannot come from a datagen report.\n//!\n")
    out.write(f"//! {len(emission)} constant emitters, {len(lit_emission)} lit-conditional,\n")
    out.write(f"//! {len(no_occlude)} non-occluding blocks.\n")
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
        f"{len(damp_over)} damp-overrides, {len(unparsed)} approximated",
        file=sys.stderr,
    )
    for u in unparsed:
        print(f"  approx: {u}", file=sys.stderr)


if __name__ == "__main__":
    main()
