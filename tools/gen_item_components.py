"""Machine-extract every item's **prototype component set** into a Rust table
(`crates/rewo-data/src/item_components_table.rs`).

Why this exists
---------------
The advanced tooltip (F3+H) ends with

    builder.accept(Component.translatable("item.components", count)...)
    where  int count = this.components.size();

and `this.components` is a `PatchedDataComponentMap`, **not** the patch. Its
`size()` is

    int size = this.prototype.size();
    for (entry : this.patch) {
       boolean inPatch     = entry.getValue().isPresent();
       boolean inPrototype = this.prototype.has(entry.getKey());
       if (inPatch != inPrototype) size += inPatch ? 1 : -1;
    }

so the number a player sees is dominated by the **prototype**: a plain dirt
stack arrives with an empty patch and still reads "13 component(s)". Reading
the patch's own entry count instead would show `0` for almost every stack in
the game.

That means the client needs two things the wire never sends: how many
components an item's prototype has, and *which* ones — the second because a
patch entry only changes the count when its presence differs from the
prototype's.

The shape of the data
---------------------
Twelve components are on **every** one of 26.2's 1,537 items
(`DataComponents.COMMON_ITEM_COMPONENTS` plus what `Item.Properties` always
sets), and only 47 more appear at all. So the table stores the twelve once and
a 64-bit mask of the rest per item — 1,537 rows of `(name, mask)` rather than
19,356 strings.

Every item is listed, including the 1,091 whose mask is zero, because
membership is itself an answer: an item the table does not know gets **no**
`item.components` line rather than a guessed one, the same fail-closed rule
M22's unresolvable item models take.

Ground truth is the datagen item-component report, never community docs
(REWO_PLAN §11):

    <APPDATA>/EwoClient/rewo/<version>/datagen/generated/reports/
        minecraft/components/item/<item>.json

plus one decompiled class, read to pin the meaning of the count:

    net/minecraft/core/component/PatchedDataComponentMap.java

Re-run after a version bump:

    python tools/gen_item_components.py

Fails loud rather than defaulting: a missing report tree, a report that is not
an object, more than 64 non-universal component names (the mask would silently
truncate), or a `PatchedDataComponentMap.size()` that no longer matches the
formula this table is built for are all hard errors.
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
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "item_components_table.rs")

# The mask is a `u64`, so this is the ceiling on non-universal component names.
MASK_BITS = 64


def die(msg):
    print(f"gen_item_components: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        die(f"cannot read {path}: {e}")
        raise


def check_size_formula():
    """Pin `PatchedDataComponentMap.size()`'s shape.

    The whole table exists to feed that method. If a version rewrote it — to
    count the patch, say, or to stop subtracting removals — this table would
    still generate cleanly and every advanced tooltip would be quietly wrong.
    """
    src = read(os.path.join(DECOMP, "core", "component",
                            "PatchedDataComponentMap.java"))
    body = re.search(r"public int size\(\)\s*\{(.*?)\n   \}", src, re.S)
    if not body:
        die("could not find `public int size()` in PatchedDataComponentMap")
    b = body.group(1)
    for needle in ["this.prototype.size()", "isPresent()",
                   "this.prototype.has(", "inPatch != inPrototype",
                   "inPatch ? 1 : -1"]:
        if needle not in b:
            die(f"PatchedDataComponentMap.size() no longer contains "
                f"`{needle}` — the merged-count rule this table feeds has "
                f"changed and the emitted numbers are unverified")


def load_reports():
    if not os.path.isdir(REPORT):
        die(f"no item component report at {REPORT} — run datagen first")
    items = {}
    for entry in sorted(os.listdir(REPORT)):
        if not entry.endswith(".json"):
            continue
        doc = json.loads(read(os.path.join(REPORT, entry)))
        comps = doc.get("components")
        if not isinstance(comps, dict):
            die(f"{entry}: no `components` object")
        items[f"minecraft:{entry[:-len('.json')]}"] = sorted(comps.keys())
    if not items:
        die(f"{REPORT} holds no reports")
    return items


def main():
    check_size_formula()
    items = load_reports()

    every = None
    seen = set()
    for names in items.values():
        seen.update(names)
        every = set(names) if every is None else (every & set(names))
    universal = sorted(every)
    extra = sorted(seen - every)
    if len(extra) > MASK_BITS:
        die(f"{len(extra)} non-universal component names but the mask is only "
            f"{MASK_BITS} bits wide — widen it or the table truncates silently")

    bit = {name: i for i, name in enumerate(extra)}
    rows = []
    for item, names in sorted(items.items()):
        mask = 0
        for n in names:
            if n in bit:
                mask |= 1 << bit[n]
        rows.append((item, mask))

    total_entries = sum(len(n) for n in items.values())
    with_extras = sum(1 for _, m in rows if m)
    hist = {}
    for names in items.values():
        hist[len(names)] = hist.get(len(names), 0) + 1
    hist_txt = ", ".join(f"{k}: {v}" for k, v in sorted(hist.items()))

    # The two numbers the emitted test pins, measured rather than written by
    # hand — the first draft of that test guessed 13 for dirt and was wrong.
    pinned = {}
    for name in ("minecraft:dirt", "minecraft:diamond_sword"):
        if name not in items:
            die(f"{name} is not in the report — the emitted test pins its count")
        pinned[name] = len(items[name])

    lines = [f'    ("{item}", 0x{mask:x}),' for item, mask in rows]
    uni = "\n".join(f'    "{n}",' for n in universal)
    ext = "\n".join(f'    "{n}",' for n in extra)
    body = "\n".join(lines)

    text = f'''//! Every item's **prototype component set** — GENERATED, do not edit.
//!
//! Regenerate with `python tools/gen_item_components.py` after a version bump.
//!
//! Source: the datagen per-item component report for {VERSION}. Of
//! {len(items)} items, {len(universal)} components are on every one of them and
//! {len(extra)} more appear at all; {total_entries} prototype entries in total
//! ({with_extras} items carry at least one non-universal component). Set-size
//! histogram: {hist_txt}.
//!
//! # Why the client needs this
//!
//! `ItemStack.addDetailsToTooltip`'s advanced block ends with
//! `Component.translatable("item.components", this.components.size())`, and
//! `this.components` is the **merged** `PatchedDataComponentMap` — prototype
//! plus patch — not the patch. A plain dirt stack arrives with an empty patch
//! and still reads "{pinned['minecraft:dirt']} component(s)". Counting the
//! patch's own entries would print `0` for nearly every stack in the game.
//!
//! `PatchedDataComponentMap.size()` adjusts the prototype's count by one for
//! each patch entry whose *presence* differs from the prototype's, which is
//! why the names are here and not just the count: an entry that overrides a
//! component the prototype already has changes nothing.
//!
//! # Membership is an answer
//!
//! Every item is listed, including the {len(items) - with_extras} whose mask is
//! zero. An item the table does not know returns `None` and the tooltip drops
//! the `item.components` line rather than guessing {len(universal)} — the same
//! fail-closed rule the unresolvable item models take.

/// The components on **every** item — `DataComponents.COMMON_ITEM_COMPONENTS`
/// plus what `Item.Properties` always sets. Stored once rather than in every
/// row.
pub const BASE_COMPONENTS: &[&str] = &[
{uni}
];

/// Every other component name a prototype carries, in the order the mask's
/// bits index them: bit `i` of a row's mask is `EXTRA_COMPONENTS[i]`.
pub const EXTRA_COMPONENTS: &[&str] = &[
{ext}
];

/// `(item name, bitmask over `EXTRA_COMPONENTS`)`, sorted by name so a binary
/// search finds it. A zero mask means the item's prototype is exactly
/// [`BASE_COMPONENTS`] — which is still a row, because absence from this table
/// means "unknown item", not "no extras".
pub const ITEM_COMPONENTS: &[(&str, u64)] = &[
{body}
];

fn mask_of(item: &str) -> Option<u64> {{
    ITEM_COMPONENTS
        .binary_search_by(|(name, _)| (*name).cmp(item))
        .ok()
        .map(|i| ITEM_COMPONENTS[i].1)
}}

/// `prototype.size()` — how many components an item carries before any patch.
///
/// `None` for an item this build's table does not know, which is a version
/// skew rather than a zero.
pub fn prototype_component_count(item: &str) -> Option<i32> {{
    mask_of(item).map(|m| BASE_COMPONENTS.len() as i32 + m.count_ones() as i32)
}}

/// `prototype.has(type)` — whether an item's prototype carries a component.
///
/// This is what decides whether a patch entry *changes* the merged count: an
/// addition the prototype already has is an override and adds nothing, and a
/// removal of something the prototype never had takes nothing away.
pub fn prototype_has_component(item: &str, component: &str) -> Option<bool> {{
    let mask = mask_of(item)?;
    if BASE_COMPONENTS.contains(&component) {{
        return Some(true);
    }}
    Some(match EXTRA_COMPONENTS.iter().position(|c| *c == component) {{
        Some(i) => mask & (1 << i) != 0,
        None => false,
    }})
}}

#[cfg(test)]
mod tests {{
    use super::*;

    /// The table is binary-searched, so an unsorted regeneration would answer
    /// `None` for real items.
    #[test]
    fn the_table_is_sorted_and_complete() {{
        assert!(ITEM_COMPONENTS.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(ITEM_COMPONENTS.len(), {len(items)});
        assert_eq!(BASE_COMPONENTS.len(), {len(universal)});
        assert_eq!(EXTRA_COMPONENTS.len(), {len(extra)});
        assert!(EXTRA_COMPONENTS.windows(2).all(|w| w[0] < w[1]));
    }}

    /// The two numbers a player actually sees, read out of the report by the
    /// generator. A plain block carries only the universal set; a sword adds
    /// {pinned['minecraft:diamond_sword'] - len(universal)}.
    #[test]
    fn the_prototype_count_is_the_number_the_tooltip_prints() {{
        assert_eq!(
            prototype_component_count("minecraft:dirt"),
            Some({pinned['minecraft:dirt']})
        );
        assert_eq!(
            prototype_component_count("minecraft:diamond_sword"),
            Some({pinned['minecraft:diamond_sword']})
        );
    }}

    /// Absence is a distinct answer from zero extras — the first suppresses
    /// the tooltip line, the second prints the base count.
    #[test]
    fn an_unknown_item_is_not_a_zero() {{
        assert_eq!(prototype_component_count("minecraft:not_an_item"), None);
        assert_eq!(prototype_has_component("minecraft:not_an_item", "minecraft:damage"), None);
    }}

    /// The membership half. `damage` is on a sword's prototype and not on
    /// dirt's, which is the difference between a patched `damage` costing the
    /// count nothing and costing it one.
    #[test]
    fn membership_separates_an_override_from_an_addition() {{
        assert_eq!(
            prototype_has_component("minecraft:diamond_sword", "minecraft:damage"),
            Some(true)
        );
        assert_eq!(
            prototype_has_component("minecraft:dirt", "minecraft:damage"),
            Some(false)
        );
        // A universal one is on everything, mask or no mask.
        assert_eq!(
            prototype_has_component("minecraft:dirt", "minecraft:rarity"),
            Some(true)
        );
        // A name no prototype carries — `minecraft:custom_name` is only ever
        // a patch — is absent from every item rather than unknown.
        assert_eq!(
            prototype_has_component("minecraft:dirt", "minecraft:custom_name"),
            Some(false)
        );
    }}
}}
'''
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"gen_item_components: {len(items)} items, {len(universal)} universal "
          f"+ {len(extra)} other component names, {total_entries} prototype "
          f"entries -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
