"""Generate crates/rewo-data/src/stonecutter_table.rs from the jar's recipes.

Two tables from one walk: the accepted-input set the **quick-move** needs
(M93b) and the ordered recipe list the **widget** needs (M93s).

# M93b recorded the list as blocked on `update_recipes`. It is not.

That was the third time this arc a class-C claim did not survive contact with
the decompile (M91's furnace recipes, M93's merchant quick-move). The list is
jar-derivable for exactly M91's reason, and the hard part is not the contents
but the ORDER.

# The order is the whole problem, and it is reproducible

A stonecutter click sends an *index* (`container_button_click`), which the
server resolves against `recipesForInput`:

    entries.stream().filter(e -> e.input.test(input)).toList()

A filter, so it preserves the master list's order. Reproduce that order wrongly
and every click selects a different recipe than the one shown, with no error
anywhere — the server cheerfully cuts the wrong block. This is M64's
alphabetisation trap in a new place.

The master list is built by `RecipeManager.finalizeRecipeLoading` walking
`this.recipes.values()` — a Guava `ImmutableMap`'s values, insertion ordered —
and the insertion order comes from `RecipeManager.prepare`:

    SortedMap<Identifier, Recipe<?>> recipes = new TreeMap<>();
    SimpleJsonResourceReloadListener.scanDirectory(manager, RECIPE_LISTER, …, recipes);

so it is sorted by `Identifier`, and `Identifier.compareTo` is

    int result = this.path.compareTo(o.path);
    if (result == 0) result = this.namespace.compareTo(o.namespace);

**path first, then namespace** — not the combined `namespace:path` string. For
an all-vanilla datapack every namespace is `minecraft` so it reduces to the file
stem, but a datapack adding recipes under another namespace would *interleave*
by path rather than append.

We sort by the stem explicitly rather than by the filename. Sorting `"a.json"`
against `"a_b.json"` happens to give the same answer only because `.` (0x2E) is
below every character `[a-z0-9_]` uses; it stops being the same answer the
moment a name uses one that is not. Java's `String.compareTo` is UTF-16
code-unit order, which for ASCII matches Python's default — asserted below
rather than assumed.

# What the widget needs beyond the order

`SingleInputEntry(Ingredient input, SelectableRecipe recipe)`, and the button
draws `recipe.optionDisplay().resolveForFirstStack(context)` — the result
stack. All 319 vanilla stonecutting ingredients are plain item strings (no
tags, no lists), asserted below, so one item per entry is exact rather than a
simplification. Result counts are carried even though the button does **not**
draw them: `extractRecipes` calls `graphics.item`, which renders the model
alone, and never `itemDecorations`.

`finalizeRecipeLoading` also filters on `isIngredientEnabled(enabledFlags, …)`
and `resultDisplay().isEnabled(enabledFlags)`; with vanilla's default feature
flags nothing is filtered, which is why this walk keeps everything.

# The caveat, unchanged from M91/M93b

The real set arrives in `update_recipes`, which Rewo does not decode. **A
datapack that adds, removes or reorders a stonecutting recipe makes this wrong
with no error anywhere** — and for the list, "wrong" now includes cutting a
different block than the one clicked. Same trade M19 makes reading
`ItemTags.SPEARS` from the jar, M42 the enchantment tags, and M91 the smelting
sets.

Run:  python tools/gen_stonecutter_inputs.py
"""

from __future__ import annotations

import json
import os
import sys

from recipe_ingredients import RECIPES, die, registered_items  # tools/ is sys.path[0]

OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "rewo-data", "src", "stonecutter_table.rs",
)

TOOL = "gen_stonecut"
# 319 in 26.2. Floored just under, so a collapse fails and a handful of new
# vanilla recipes do not. See gen_smelting_inputs.py for why the guard is here
# rather than as a `.len()` assertion in the generated file.
FLOOR = 300


def main() -> int:
    if not os.path.isdir(RECIPES):
        die(TOOL, f"no recipe data at {RECIPES}")
    items = registered_items(TOOL)

    # Sort by the Identifier's PATH — the file stem — not by the filename.
    stems = sorted(f[:-5] for f in os.listdir(RECIPES) if f.endswith(".json"))
    if not all(c.isascii() for s in stems for c in s):
        die(TOOL, "a recipe id is non-ASCII; Python's sort no longer matches Java's")

    rows: list[tuple[str, str, int]] = []
    for stem in stems:
        r = json.load(open(os.path.join(RECIPES, stem + ".json"), encoding="utf-8"))
        if r.get("type") != "minecraft:stonecutting":
            continue
        ing = r.get("ingredient")
        if not isinstance(ing, str) or ing.startswith("#"):
            # A tag or a list would need a set per entry. Vanilla has neither,
            # so fail loudly rather than silently modelling only one item.
            die(TOOL, f"{stem}: ingredient {ing!r} is not a plain item id")
        res = r.get("result")
        if not isinstance(res, dict) or "id" not in res:
            die(TOOL, f"{stem}: unrecognised result {res!r}")
        inp = ing if ":" in ing else "minecraft:" + ing
        out_id = res["id"] if ":" in res["id"] else "minecraft:" + res["id"]
        for i in (inp, out_id):
            if i not in items:
                die(TOOL, f"{stem}: {i} is not a registered item")
        rows.append((inp, out_id, int(res.get("count", 1))))

    if len(rows) < FLOOR:
        die(TOOL, f"{len(rows)} stonecutting recipes, expected at least {FLOOR}")

    # The accepted-input set is DERIVED from the same walk, so the two tables
    # cannot drift. M93r's sweep found a colour table duplicated across two
    # crates with nothing asserting the copies agreed; this is that hazard one
    # file over, removed by construction and then asserted anyway.
    acc: list[str] = []
    for inp, _, _ in rows:
        if inp not in acc:
            acc.append(inp)

    recipe_rows = "\n".join(
        f'    Cut {{ input: "{i}", result: "{o}", count: {c} }},' for i, o, c in rows
    )
    input_rows = "\n".join(f'    "{i}",' for i in acc)
    counts = sorted({c for _, _, c in rows})
    counts_lit = "vec![" + ", ".join(str(c) for c in counts) + "]"
    two = sum(1 for _, _, c in rows if c == 2)

    out = f'''//! What a stonecutter accepts and what it offers — GENERATED by
//! `tools/gen_stonecutter_inputs.py`.
//!
//! Do not edit. Re-run the generator after a version bump. (It emits its own
//! tests, so a re-run reproduces this file whole — `smelting_table.rs` used to
//! carry hand-added tests its generator did not emit, which meant following
//! that same instruction silently deleted them.)
//!
//! `StonecutterMenu.quickMoveStack`'s third branch is
//! `level.recipeAccess().stonecutterRecipes().acceptsInput(stack)`, which is a
//! `SelectableRecipe.SingleInputSet` and **not** a `RecipePropertySet` — that
//! registry has seven keys and the stonecutter is not one of them. Membership
//! still reduces to the union of every stonecutting recipe's ingredient,
//! because `Ingredient.test` is `input.is(this.values)`: item identity, no
//! components.
//!
//! # The list's ORDER is part of the wire contract
//!
//! A click sends an **index**, which the server resolves against
//! `selectByInput` — a *filter*, so it preserves the master list's order. That
//! order is `RecipeManager.prepare`'s `SortedMap<Identifier, Recipe<?>> = new
//! TreeMap<>()`, and `Identifier.compareTo` compares the **path first, then
//! the namespace**, not the combined `namespace:path`. Reproduce it wrongly
//! and every click cuts a different block, with no error anywhere.
//!
//! # The caveat
//!
//! The real set arrives in `update_recipes`, which Rewo does not decode. **A
//! datapack that adds, removes or reorders a stonecutting recipe makes this
//! wrong with no error anywhere.** Same trade M19 makes reading
//! `ItemTags.SPEARS` from the jar, M42 the enchantment tags, and M91 the
//! smelting sets. M93b recorded the LIST as blocked on `update_recipes`; it is
//! not, for the same reason M91's furnace recipes were not.

/// One `SelectableRecipe.SingleInputEntry<StonecutterRecipe>`.
///
/// `count` is carried even though the button never draws it: `extractRecipes`
/// calls `graphics.item`, which renders the model alone and never
/// `itemDecorations`. A tooltip needs the whole stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {{
    /// The ingredient. All {len(rows)} vanilla entries are a plain item id —
    /// no tags, no lists — which the generator asserts rather than assumes.
    pub input: &'static str,
    pub result: &'static str,
    pub count: u8,
}}

/// Every stonecutting recipe, in `RecipeMap.values()` order — {len(rows)}
/// entries. **The index a click sends indexes the filtered view of this
/// list**, so this order is part of the wire contract.
pub static STONECUTTER_RECIPES: &[Cut] = &[
{recipe_rows}
];

/// Every item a stonecutter accepts in its input slot — {len(acc)} items.
///
/// Derived from [`STONECUTTER_RECIPES`] by the same generator walk, so the two
/// cannot drift; `acceptsInput` is `entries.stream().anyMatch(…)` over exactly
/// that list.
pub static STONECUTTER_INPUT: &[&str] = &[
{input_rows}
];

/// `SelectableRecipe.SingleInputSet.selectByInput` — the entries whose
/// ingredient matches, **in master-list order**.
///
/// ```java
/// return new SingleInputSet<>(this.entries.stream().filter(e -> e.input.test(input)).toList());
/// ```
///
/// `Ingredient.test` is `input.is(this.values)`: item identity, no components.
pub fn select_by_input(item: &str) -> Vec<&'static Cut> {{
    STONECUTTER_RECIPES.iter().filter(|c| c.input == item).collect()
}}

/// `SingleInputSet.acceptsInput` — whether a stonecutter routes this item to
/// its input slot on a shift-click.
///
/// Note this is **not** the input slot's `mayPlace`: that slot is a bare
/// `Slot` with no override, so an ordinary click can put anything in it. The
/// predicate exists only in `quickMoveStack`.
pub fn accepts_input(item: &str) -> bool {{
    STONECUTTER_INPUT.contains(&item)
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn the_obvious_stonecuttable_blocks_are_accepted() {{
        assert!(accepts_input("minecraft:stone"));
        assert!(accepts_input("minecraft:andesite"));
        assert!(accepts_input("minecraft:copper_block"));
    }}

    #[test]
    fn an_item_with_no_stonecutting_recipe_is_refused() {{
        // The negative is what the routing actually turns on: a false here is
        // what sends the stack to the hotbar instead of the input slot.
        assert!(!accepts_input("minecraft:stick"));
        assert!(!accepts_input("minecraft:beef"));
        // A stonecutter RESULT is not itself an input — quartz stairs are cut
        // from the block, and cutting the stairs again is not a recipe.
        assert!(!accepts_input("minecraft:andesite_wall"));
    }}

    #[test]
    fn the_stonecutter_set_is_not_the_furnace_set() {{
        // If the two tables were wired to the same data the routing would
        // still "work" and be wrong in both menus. The pair must be genuinely
        // disjoint, which `stone` is NOT — it cuts into slabs and smelts into
        // smooth stone, so the first draft of this test asserted stone was
        // unsmeltable and failed against correct code.
        assert!(accepts_input("minecraft:andesite"));
        assert_eq!(crate::smelting_table::accepts(14, "minecraft:andesite"), Some(false));
        assert!(!accepts_input("minecraft:beef"));
        assert_eq!(crate::smelting_table::accepts(14, "minecraft:beef"), Some(true));
    }}

    #[test]
    fn an_item_that_is_both_is_routed_by_the_MENU_not_by_the_item() {{
        // M91's log — fuel AND smeltable — one menu over. Cobblestone cuts
        // into stairs and smelts into stone, so a single "what is this item
        // for" flag could not route it: the furnace sends it to the ingredient
        // slot and the stonecutter to the input slot, and both are right.
        assert!(accepts_input("minecraft:cobblestone"));
        assert_eq!(crate::smelting_table::accepts(14, "minecraft:cobblestone"), Some(true));
    }}

    #[test]
    fn the_two_tables_cannot_disagree() {{
        // Both come from one generator walk, and this is the guard M93r's
        // sweep says to write anyway: a derived table with nothing asserting
        // the derivation is one edit away from silently diverging.
        let mut derived: Vec<&str> = Vec::new();
        for c in STONECUTTER_RECIPES {{
            if !derived.contains(&c.input) {{
                derived.push(c.input);
            }}
        }}
        assert_eq!(derived, STONECUTTER_INPUT);
    }}

    #[test]
    fn select_by_input_preserves_master_order() {{
        // The click sends an index into THIS list, so a filter that reordered
        // — grouping by result, say, or sorting by name — would select a
        // different recipe than the one drawn, silently.
        let cuts = select_by_input("minecraft:andesite");
        assert!(cuts.len() > 1, "andesite cuts several ways");
        let mut it = STONECUTTER_RECIPES.iter();
        for c in &cuts {{
            assert!(it.any(|m| std::ptr::eq(m, *c)), "out of master order");
        }}
        assert!(cuts.iter().all(|c| c.input == "minecraft:andesite"));
    }}

    #[test]
    fn an_unaccepted_item_offers_nothing() {{
        assert!(select_by_input("minecraft:dirt").is_empty());
        // …and cutting is one-directional: a result is not thereby an input.
        assert!(select_by_input("minecraft:andesite_stairs").is_empty());
    }}

    #[test]
    fn results_carry_their_count_even_though_the_button_hides_it() {{
        // `extractRecipes` calls `graphics.item`, which draws the model and
        // nothing else. A naive reuse of the inventory's slot draw would put a
        // "2" on every slab button.
        assert_eq!(STONECUTTER_RECIPES.iter().filter(|c| c.count == 2).count(), {two});
        let counts: std::collections::BTreeSet<u8> =
            STONECUTTER_RECIPES.iter().map(|c| c.count).collect();
        assert_eq!(counts.into_iter().collect::<Vec<_>>(), {counts_lit});
    }}
}}
'''
    open(OUT, "w", encoding="utf-8", newline="\n").write(out)
    print(
        f"[{TOOL}] {len(rows)} recipes -> {len(acc)} accepted inputs, "
        f"counts {counts} -> {os.path.relpath(OUT)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
