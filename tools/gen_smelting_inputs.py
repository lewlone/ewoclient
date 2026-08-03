"""Generate crates/rewo-data/src/smelting_table.rs from the jar's own recipes.

`AbstractFurnaceMenu.canSmelt` is `acceptedInputs.test(stack)`, where
`acceptedInputs` is `recipeAccess.propertySet(allowedInputs)` and each furnace
passes its own:

    FurnaceMenu       -> RecipePropertySet.FURNACE_INPUT       (smelting)
    BlastFurnaceMenu  -> RecipePropertySet.BLAST_FURNACE_INPUT (blasting)
    SmokerMenu        -> RecipePropertySet.SMOKER_INPUT        (smoking)

On the client those sets arrive in `update_recipes`, which Rewo does not decode
— but for **vanilla** their contents are exactly the ingredient sets of the
jar's own `data/minecraft/recipe/*.json`, and the jar is already the source for
`ItemTags.SPEARS` (M19) and the enchantment tags (M42).

**The caveat that comes with that, and it is the same one M19 and M42 carry:**
a datapack that adds or removes a smelting recipe makes this table wrong, with
no error anywhere — a shift-clicked item would route to the player's inventory
instead of the ingredient slot, or the reverse. `update_recipes` is the
authoritative source and is class C. This is the vanilla answer, not the
server's.

Ingredients are either an item id or a `#tag`, and tags nest, so expansion
recurses — the same shape `gen_fuel_values.py` handles.

Run:  python tools/gen_smelting_inputs.py
Exit: 0 on success; 1 on an unresolvable tag or item, or on a recipe count that
      has collapsed — a short set is invisible, since the item simply refuses
      to route to the ingredient slot.
"""

from __future__ import annotations

import json
import os
import sys

BASE = os.path.expandvars(r"%APPDATA%/EwoClient/rewo/26.2")
RECIPES = os.path.join(BASE, "decompiled/data/minecraft/recipe")
TAGS = os.path.join(BASE, "decompiled/data/minecraft/tags/item")
REGISTRIES = os.path.join(BASE, "datagen/generated/reports/registries.json")
OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "rewo-data", "src", "smelting_table.rs",
)

# (recipe type, the RecipePropertySet it feeds, a floor the count must clear)
KINDS = [
    ("minecraft:smelting", "FURNACE", 60),
    ("minecraft:blasting", "BLAST_FURNACE", 20),
    ("minecraft:smoking", "SMOKER", 5),
]


def die(msg: str) -> None:
    print(f"[gen_smelt] FAIL — {msg}")
    sys.exit(1)


def expand_tag(name: str, seen: set[str] | None = None) -> list[str]:
    seen = seen or set()
    if name in seen:
        die(f"tag cycle at {name}")
    seen.add(name)
    p = os.path.join(TAGS, name.split(":")[-1] + ".json")
    if not os.path.exists(p):
        die(f"no tag file for {name}")
    out: list[str] = []
    for v in json.load(open(p, encoding="utf-8"))["values"]:
        entry = v["id"] if isinstance(v, dict) else v
        if entry.startswith("#"):
            out += expand_tag(entry[1:], set(seen))
        else:
            out.append(entry if ":" in entry else "minecraft:" + entry)
    return out


def ingredients(value) -> list[str]:
    """A recipe's `ingredient` field: an id, a `#tag`, or a list of either."""
    if isinstance(value, list):
        out: list[str] = []
        for v in value:
            out += ingredients(v)
        return out
    if isinstance(value, dict):
        # Older/alternate shapes carry the id under a key.
        for k in ("item", "tag", "id"):
            if k in value:
                v = value[k]
                return ingredients(("#" + v) if k == "tag" else v)
        die(f"unrecognised ingredient object: {value!r}")
    if not isinstance(value, str):
        die(f"unrecognised ingredient: {value!r}")
    return expand_tag(value[1:]) if value.startswith("#") else [
        value if ":" in value else "minecraft:" + value
    ]


def main() -> int:
    if not os.path.isdir(RECIPES):
        die(f"no recipe data at {RECIPES}")
    items = set(json.load(open(REGISTRIES, encoding="utf-8"))["minecraft:item"]["entries"])

    sets: dict[str, list[str]] = {}
    for kind, name, floor in KINDS:
        found = 0
        acc: list[str] = []
        for fn in sorted(os.listdir(RECIPES)):
            if not fn.endswith(".json"):
                continue
            r = json.load(open(os.path.join(RECIPES, fn), encoding="utf-8"))
            if r.get("type") != kind:
                continue
            found += 1
            if "ingredient" not in r:
                die(f"{fn}: a {kind} recipe with no ingredient")
            for i in ingredients(r["ingredient"]):
                if i not in items:
                    die(f"{fn}: ingredient {i} is not a registered item")
                if i not in acc:
                    acc.append(i)
        if found < floor:
            die(f"{kind}: {found} recipes, expected at least {floor} — the data moved")
        sets[name] = acc
        print(f"[gen_smelt] {name}: {found} recipes -> {len(acc)} accepted inputs")

    def block(name: str) -> str:
        rows = "\n".join(f'    "{i}",' for i in sets[name])
        return (
            f"/// `RecipePropertySet.{name}_INPUT` — {len(sets[name])} items.\n"
            f"pub static {name}_INPUT: &[&str] = &[\n{rows}\n];\n"
        )

    out = f'''//! What each furnace will accept — GENERATED by `tools/gen_smelting_inputs.py`.
//!
//! Do not edit. Re-run the generator after a version bump.
//!
//! `AbstractFurnaceMenu.canSmelt` is `acceptedInputs.test(stack)`, and each
//! furnace passes its own `RecipePropertySet`: `FURNACE_INPUT` (smelting),
//! `BLAST_FURNACE_INPUT` (blasting), `SMOKER_INPUT` (smoking). These are the
//! ingredient sets of the jar's own recipes, with tags expanded.
//!
//! # The caveat
//!
//! On the client those sets really arrive in `update_recipes`, which Rewo does
//! not decode. **A datapack that adds or removes a smelting recipe makes this
//! wrong with no error anywhere** — a shift-clicked item routes to the player's
//! inventory instead of the ingredient slot, or the reverse. This is the
//! vanilla answer, not the server's, and it is the same trade M19 makes reading
//! `ItemTags.SPEARS` from the jar and M42 reading the enchantment tags.

{block("FURNACE")}
{block("BLAST_FURNACE")}
{block("SMOKER")}
/// Whether a furnace of the given `minecraft:menu` id accepts `item` as an
/// ingredient, or `None` if that menu is not a furnace.
pub fn accepts(menu_protocol_id: i32, item: &str) -> Option<bool> {{
    let set: &[&str] = match menu_protocol_id {{
        10 => BLAST_FURNACE_INPUT,
        14 => FURNACE_INPUT,
        22 => SMOKER_INPUT,
        _ => return None,
    }};
    Some(set.contains(&item))
}}
'''
    open(OUT, "w", encoding="utf-8", newline="\n").write(out)
    print(f"[gen_smelt] wrote {os.path.relpath(OUT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
