"""Generate crates/rewo-data/src/fuel_table.rs from the 26.2 decompile + jar tags.

`AbstractFurnaceMenu.quickMoveStack` routes a shift-clicked stack to the fuel
slot when `isFuel(stack)`, which is `fuelValues.burnDuration(stack) > 0`. That
table is `FuelValues.vanillaBurnTimes`, a builder chain in the decompile whose
tag arguments resolve against the jar's own `data/minecraft/tags/item/*.json`.

Unlike the menu slot layouts (see `check_menu_layouts.py` for why those are
hand-written and checked), this IS a generator: the chain is one regular idiom,
and the only cross-file work is expanding tags, which is data rather than code.

Three semantics from `FuelValues.Builder`, each of which changes the result:

* The map is an `Object2IntLinkedOpenHashMap` and `putInternal` uses `put`, so
  **a later `.add` overwrites an earlier one for the same item**. Order matters.
* `.add(tag, time)` expands the tag; tags **nest** (`logs` is three tag
  references, no items of its own), so expansion recurses.
* **`.remove(ItemTags.NON_FLAMMABLE_WOOD)` runs last** and deletes whatever the
  `logs`/`planks` families added for crimson and warped. Skipping it makes
  warped planks burnable, which they are not.

`baseUnit` is **200** — `vanillaBurnTimes`'s two-argument form passes it.

Run:  python tools/gen_fuel_values.py
Exit: 0 on success; 1 on any unresolvable tag, item or expression. It fails
      loud rather than emitting a short table, because a missing fuel is
      invisible — the item simply refuses to route to the fuel slot.
"""

from __future__ import annotations

import json
import os
import re
import sys

BASE = os.path.expandvars(r"%APPDATA%/EwoClient/rewo/26.2")
SRC = os.path.join(
    BASE, "decompiled/net/minecraft/world/level/block/entity/FuelValues.java"
)
TAGS = os.path.join(BASE, "decompiled/data/minecraft/tags/item")
REGISTRIES = os.path.join(BASE, "datagen/generated/reports/registries.json")
OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "rewo-data", "src", "fuel_table.rs",
)

BASE_UNIT = 200


def die(msg: str) -> None:
    print(f"[gen_fuel] FAIL — {msg}")
    sys.exit(1)


def snake(constant: str) -> str:
    return constant.lower()


def expand_tag(name: str, seen: set[str] | None = None) -> list[str]:
    """A tag's items, following nested `#tag` references."""
    seen = seen or set()
    if name in seen:
        die(f"tag cycle at {name}")
    seen.add(name)
    p = os.path.join(TAGS, name.split(":")[-1] + ".json")
    if not os.path.exists(p):
        die(f"no tag file for {name} ({p})")
    out: list[str] = []
    for v in json.load(open(p, encoding="utf-8"))["values"]:
        entry = v["id"] if isinstance(v, dict) else v
        if entry.startswith("#"):
            out += expand_tag(entry[1:], set(seen))
        else:
            out.append(entry if ":" in entry else "minecraft:" + entry)
    return out


def evaluate(expr: str) -> int:
    """`baseUnit * 3 / 2` and friends, with **Java's operator precedence**.

    Not left-to-right, which an earlier cut of this assumed and which is wrong:
    `1 + baseUnit * 20` is `1 + 4000 = 4001`, not `(1 + 200) * 20 = 4020`. The
    tell is that the other `1 + ...` term, `1 + baseUnit / 3`, gives 67 under
    *either* reading — so checking that one alone confirms nothing, and only
    `dried_kelp_block` separates them. `*` and `/` bind tighter than `+` and
    associate left-to-right among themselves, and the division truncates.
    """
    e = expr.strip()
    if not re.fullmatch(r"[\w\s*/+]+", e):
        die(f"unexpected burn-time expression: {expr!r}")
    total = 0
    for term in e.split("+"):
        tokens = re.findall(r"baseUnit|\d+|[*/]", term)
        if not tokens:
            die(f"unparsed term {term!r} in {expr!r}")
        it = iter(tokens)
        first = next(it)
        value = BASE_UNIT if first == "baseUnit" else int(first)
        while (op := next(it, None)) is not None:
            rhs_tok = next(it, None)
            if rhs_tok is None:
                die(f"dangling operator in {expr!r}")
            rhs = BASE_UNIT if rhs_tok == "baseUnit" else int(rhs_tok)
            # Java int division truncates toward zero; these are all positive.
            value = value * rhs if op == "*" else value // rhs
        total += value
    return total


def main() -> int:
    if not os.path.exists(SRC):
        die(f"no decompile at {SRC}")
    text = open(SRC, encoding="utf-8", errors="replace").read()
    body = re.search(
        r"vanillaBurnTimes\(final HolderLookup\.Provider registries, "
        r"final FeatureFlagSet enabledFeatures, final int baseUnit\) \{(.*?)\n   \}",
        text,
        re.S,
    )
    if not body:
        die("could not find the three-argument vanillaBurnTimes")
    chain = body.group(1)

    # Every registered item, so a generated name can be validated rather than
    # trusted — the same rule `gen_block_light.py` follows.
    if not os.path.exists(REGISTRIES):
        die(f"no registries report at {REGISTRIES}")
    items = set(json.load(open(REGISTRIES, encoding="utf-8"))["minecraft:item"]["entries"])

    values: dict[str, int] = {}
    steps = re.findall(
        r"\.(add|remove)\(\s*(Items|Blocks|ItemTags)\.(\w+)\s*(?:,\s*([^)]+))?\)", chain
    )
    if len(steps) < 40:
        die(f"only {len(steps)} builder steps parsed — the idiom has changed")
    for kind, holder, name, expr in steps:
        tag = holder == "ItemTags"
        if kind == "remove":
            if not tag:
                die(f"remove() of a non-tag: {holder}.{name}")
            for i in expand_tag("minecraft:" + snake(name)):
                values.pop(i, None)
            continue
        time = evaluate(expr)
        targets = (
            expand_tag("minecraft:" + snake(name)) if tag else ["minecraft:" + snake(name)]
        )
        for i in targets:
            if i not in items:
                die(f"{holder}.{name} produced {i}, which is not a registered item")
            # `put` overwrites — a later add wins.
            values[i] = time

    rows = "\n".join(f'    ("{k}", {v}),' for k, v in values.items())
    out = f'''//! Vanilla furnace fuel burn times — GENERATED by `tools/gen_fuel_values.py`.
//!
//! Do not edit. Re-run the generator after a version bump.
//!
//! `FuelValues.vanillaBurnTimes` with `baseUnit = {BASE_UNIT}`, its tag arguments
//! expanded against the jar's own `data/minecraft/tags/item/*.json` (which nest,
//! so the expansion recurses) and its trailing
//! `.remove(ItemTags.NON_FLAMMABLE_WOOD)` applied — without which crimson and
//! warped wood would burn, because the `logs` and `planks` families add them
//! first.
//!
//! Order is the builder's, and it is load-bearing while generating: the map is
//! insertion-ordered and `put` overwrites, so a later `.add` wins.

/// `(item id, burn duration in ticks)`, {len(values)} entries.
pub static FUEL: &[(&str, i32)] = &[
{rows}
];

/// `FuelValues.burnDuration` — 0 for anything absent, which is what
/// `Object2IntMap`'s default return value gives.
pub fn burn_duration(item: &str) -> i32 {{
    FUEL.iter()
        .find(|(k, _)| *k == item)
        .map(|(_, v)| *v)
        .unwrap_or(0)
}}

/// `AbstractFurnaceMenu.isFuel` — a positive burn duration.
pub fn is_fuel(item: &str) -> bool {{
    burn_duration(item) > 0
}}
'''
    open(OUT, "w", encoding="utf-8", newline="\n").write(out)
    print(f"[gen_fuel] wrote {len(values)} fuels to {os.path.relpath(OUT)}")
    print(f"[gen_fuel] {len(steps)} builder steps; baseUnit {BASE_UNIT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
