"""Shared recipe-ingredient extraction for the jar-derived accepted-input tables.

Extracted from `gen_smelting_inputs.py` (M91) when `gen_stonecutter_inputs.py`
(M93b) needed the same recursive tag expansion. **Two copies of a recursive tag
expander that must agree is exactly the class of silent divergence this project
hunts**, so there is one, and the extraction was proved inert by re-running the
smelting generator and getting `smelting_table.rs` back byte for byte.

What "accepted input" means differs by menu and the difference does NOT reach
here:

  * a furnace asks `RecipePropertySet.test`, which is a flat `Set<Holder<Item>>`
    built by `create(...)` flat-mapping every ingredient
  * a stonecutter asks `SelectableRecipe.SingleInputSet.acceptsInput`, which is
    `entries.stream().anyMatch(e -> e.input.test(input))`

Both reduce to "is this item in the union of the ingredients", because
`Ingredient.test` is `input.is(this.values)` — item identity, no components.
The distinction is load-bearing for `selectByInput` (which recipes a given
input offers) and inert for membership, which is all these tables answer.
"""

from __future__ import annotations

import json
import os
import sys

BASE = os.path.expandvars(r"%APPDATA%/EwoClient/rewo/26.2")
RECIPES = os.path.join(BASE, "decompiled/data/minecraft/recipe")
TAGS = os.path.join(BASE, "decompiled/data/minecraft/tags/item")
REGISTRIES = os.path.join(BASE, "datagen/generated/reports/registries.json")


def die(tag: str, msg: str) -> None:
    print(f"[{tag}] FAIL — {msg}")
    sys.exit(1)


def expand_tag(
    tool: str,
    name: str,
    seen: set[str] | None = None,
    registry: str = "item",
) -> list[str]:
    """Every id in a tag, following `#`-references.

    `registry` is the tag's own registry directory — `item` for
    `ItemTags`, `banner_pattern` for `BannerPatternTags`, and so on. It was
    hardcoded to `item` until M93o needed `no_item_required`, and M93h had
    recorded that hardcoding as one of the loom's three blockers. It is the
    only one of the three that was real.

    The name may carry a `/`, because a tag id is a path: `pattern_item/flower`
    lives at `tags/banner_pattern/pattern_item/flower.json`. So only the
    NAMESPACE is stripped, never the path.
    """
    seen = seen or set()
    key = f"{registry}:{name}"
    if key in seen:
        die(tool, f"tag cycle at {key}")
    seen.add(key)
    root = os.path.join(BASE, "decompiled/data/minecraft/tags", registry)
    p = os.path.join(root, name.split(":")[-1] + ".json")
    if not os.path.exists(p):
        die(tool, f"no {registry} tag file for {name} (looked at {p})")
    out: list[str] = []
    for v in json.load(open(p, encoding="utf-8"))["values"]:
        entry = v["id"] if isinstance(v, dict) else v
        if entry.startswith("#"):
            out += expand_tag(tool, entry[1:], set(seen), registry)
        else:
            out.append(entry if ":" in entry else "minecraft:" + entry)
    return out


def ingredients(tool: str, value) -> list[str]:
    """A recipe's `ingredient` field: an id, a `#tag`, or a list of either."""
    if isinstance(value, list):
        out: list[str] = []
        for v in value:
            out += ingredients(tool, v)
        return out
    if isinstance(value, dict):
        # Older/alternate shapes carry the id under a key.
        for k in ("item", "tag", "id"):
            if k in value:
                v = value[k]
                return ingredients(tool, ("#" + v) if k == "tag" else v)
        die(tool, f"unrecognised ingredient object: {value!r}")
    if not isinstance(value, str):
        die(tool, f"unrecognised ingredient: {value!r}")
    return (
        expand_tag(tool, value[1:])
        if value.startswith("#")
        else [value if ":" in value else "minecraft:" + value]
    )


def registered_items(tool: str) -> set[str]:
    """Every id in `minecraft:item`, so an ingredient that resolves to nothing
    fails loudly instead of silently shrinking the accepted set."""
    if not os.path.exists(REGISTRIES):
        die(tool, f"no registries report at {REGISTRIES}")
    return set(json.load(open(REGISTRIES, encoding="utf-8"))["minecraft:item"]["entries"])


def accepted_inputs(tool: str, recipe_type: str, floor: int) -> tuple[list[str], int]:
    """The union of every ingredient of one recipe type, in file order.

    `floor` is a fail-closed minimum on the recipe count. A short set is
    invisible at runtime — the item simply refuses to route to the input slot —
    so a data move that halves it must stop the generator, not be absorbed.
    """
    if not os.path.isdir(RECIPES):
        die(tool, f"no recipe data at {RECIPES}")
    items = registered_items(tool)
    found = 0
    acc: list[str] = []
    for fn in sorted(os.listdir(RECIPES)):
        if not fn.endswith(".json"):
            continue
        r = json.load(open(os.path.join(RECIPES, fn), encoding="utf-8"))
        if r.get("type") != recipe_type:
            continue
        found += 1
        if "ingredient" not in r:
            die(tool, f"{fn}: a {recipe_type} recipe with no ingredient")
        for i in ingredients(tool, r["ingredient"]):
            if i not in items:
                die(tool, f"{fn}: ingredient {i} is not a registered item")
            if i not in acc:
                acc.append(i)
    if found < floor:
        die(tool, f"{recipe_type}: {found} recipes, expected at least {floor} — the data moved")
    return acc, found
