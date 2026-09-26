"""M152's mutation battery — the smithing quick-move and the `update_recipes` decode.

Run: python tools/m152_mutate.py

Discipline, all of it earned by this project's own recorded harness failures:

* **A no-op control that must SURVIVE.** A battery run against an already-red
  tree reports KILLED for every entry and looks like a perfect score (M109's
  eight vacuous mutations). The control is entry 0 and the battery refuses to
  report if it dies.
* **Exit codes, never substrings.** `grep 'PASS'` cannot distinguish a failing
  test from a failing *build*, and M141's harness read KILLED for a run whose
  link step had failed.
* **A per-mutation timeout.** A mutant that hangs takes the battery down with
  it, its `finally` never runs, and the mutation is LEFT ON DISK (M93g, M96,
  M104). A timeout makes a hang a KILL instead of an outage.
* **Restore verified by BYTES.** `git diff --quiet` cannot tell a leftover
  mutation from ordinary uncommitted work, which is exactly the state a
  milestone in progress is in (M138a).
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INV = "crates/rewo-world/src/inventory.rs"
MENU = "crates/rewo-world/src/menu_layout.rs"
BOOK = "crates/rewo-net/src/recipe_book.rs"

# (name, file, find, replace, crate, test filter, why it must die)
MUTATIONS = [
    (
        "control: no change at all",
        INV,
        "let (result_slot, player) = (3usize, 4usize);",
        "let (result_slot, player) = (3usize, 4usize);",
        "rewo-world",
        "smithing",
        "MUST SURVIVE — if this dies the battery is grading a red tree",
    ),
    (
        "smithing: drop the cross-move arms (the M152 finding)",
        INV,
        """                return Some(vec![if slot < hotbar {
                    (hotbar..end, false)
                } else {
                    (player..hotbar, false)
                }]);
            }
            QuickMove::Beacon => {""",
        """                return Some(vec![(0..result_slot, false)]);
            }
            QuickMove::Beacon => {""",
        "rewo-world",
        "smithing",
        "a refused stack would move nothing, as if smithing were an anvil",
    ),
    (
        "smithing: guard ignores slot occupancy",
        INV,
        """                let accepted = (p.smithing_template && slots[0].is_none())
                    || (p.smithing_base && slots[1].is_none())
                    || (p.smithing_addition && slots[2].is_none());""",
        """                let accepted =
                    p.smithing_template || p.smithing_base || p.smithing_addition;""",
        "rewo-world",
        "smithing",
        "a second ingot would be claimed by an already-full slot",
    ),
    (
        "smithing: the three slots share one predicate",
        INV,
        """            SlotKind::SmithingTemplate => props.smithing_template,
            SlotKind::SmithingBase => props.smithing_base,
            SlotKind::SmithingAddition => props.smithing_addition,""",
        """            SlotKind::SmithingTemplate
            | SlotKind::SmithingBase
            | SlotKind::SmithingAddition => true,""",
        "rewo-world",
        "smithing",
        "the catch-all case: a plain click drops anything into any input slot",
    ),
    (
        "smithing: routed as a plain ItemCombiner",
        MENU,
        "            21 => QuickMove::Smithing,",
        "            21 => QuickMove::ItemCombiner { result_slot: 3 },",
        "rewo-world",
        "smithing",
        "the whole design decision, reverted",
    ),
    (
        "decode: property set read as a holderSet (count + 1)",
        BOOK,
        """        let m = r
            .varint()
            .map_err(|e| format!("update_recipes: set {i} len: {e:?}"))?;""",
        """        let m = r
            .varint()
            .map_err(|e| format!("update_recipes: set {i} len: {e:?}"))?
            - 1;""",
        "rewo-net",
        "recipe_book",
        "the headline trap: the two item-collection encodings confused",
    ),
    (
        "decode: absent key reported as empty",
        BOOK,
        """        self.property_sets
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())""",
        """        Some(
            self.property_sets
                .iter()
                .rev()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_slice())
                .unwrap_or(&[]),
        )""",
        "rewo-net",
        "recipe_book",
        "silence and an empty set would become indistinguishable",
    ),
    (
        "decode: duplicate key is first-wins",
        BOOK,
        """        self.property_sets
            .iter()
            .rev()
            .find(|(k, _)| k == key)""",
        """        self.property_sets
            .iter()
            .find(|(k, _)| k == key)""",
        "rewo-net",
        "recipe_book",
        "diverges from the HashMap vanilla decodes into",
    ),
    (
        "decode: ingredient read as a plain count (no tag form)",
        BOOK,
        """    Ok(match r.varint().map_err(|_| ())? {
        0 => IngredientSet::Tag(r.string(32767).map_err(|_| ())?),""",
        """    Ok(match r.varint().map_err(|_| ())? + 1 {
        99999 => IngredientSet::Tag(r.string(32767).map_err(|_| ())?),""",
        "rewo-net",
        "recipe_book",
        "the tag sentinel lost; every ingredient shifts by one",
    ),
]


def run(crate, filt):
    """Return True if the tests PASSED. Distinguishes a build failure from a
    test failure only in the printed reason, not in the verdict — both mean the
    mutant did not survive cleanly."""
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", crate, "--lib", filt],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=420,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    if p.returncode != 0:
        why = "build failed" if "error[E" in p.stderr or "could not compile" in p.stderr else "tests failed"
        return False, why
    if "test result: ok" not in p.stdout:
        return False, "no test result line"
    return True, "passed"


def main():
    results = []
    for name, path, find, repl, crate, filt, why in MUTATIONS:
        full = os.path.join(ROOT, path)
        original = io.open(full, encoding="utf-8", newline="").read()
        if original.count(find) != 1:
            print(f"SKIP  {name}: anchor matched {original.count(find)} times")
            results.append((name, "SKIP", why))
            continue
        try:
            io.open(full, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl)
            )
            survived, reason = run(crate, filt)
        finally:
            io.open(full, "w", encoding="utf-8", newline="").write(original)
            # Verified by BYTES, not by git.
            assert io.open(full, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
        results.append((name, verdict, why))

    print()
    control = results[0]
    if control[1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        print("The tree was already failing, so every KILLED above is vacuous.")
        return 2

    killed = sum(1 for _, v, _ in results[1:] if v == "KILLED")
    total = len(results) - 1
    print(f"control SURVIVED (battery is valid) — {killed}/{total} mutations killed")
    for name, verdict, why in results[1:]:
        if verdict != "KILLED":
            print(f"  {verdict}: {name}\n    would mean: {why}")
    return 0 if killed == total else 1


if __name__ == "__main__":
    sys.exit(main())
