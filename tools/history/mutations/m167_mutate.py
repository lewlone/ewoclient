"""M167's mutation battery — the `*shot` witness-name check.

    python tools/m167_mutate.py [lo] [hi]

Routed through `cargo test -p rewo-app --bins witness_names`, which is where
every claim lives (M158's gotcha 0d — grade through whatever you are claiming
coverage from).

**One thing about this battery is unusual and worth reading before the table.**
The check's headline assertion — *no gate file defines one witness name twice* —
**cannot be killed by loosening it**, because no gate file currently does.
Raising the threshold, widening the allow-list, or dropping the comparison
outright all leave a green suite on a healthy tree. That is the M138a
self-skip trap inverted: the assertion is unobservable precisely on the tree
where it holds.

So the battery mutates the **data** as well as the check: m2 introduces a real
duplicate into a gate file and requires the check to catch it. Without that
row, every other verdict here would be about the floors and the allow-list
rather than about the thing the milestone is for.

Discipline, inherited: a no-op control that must SURVIVE, exit codes rather
than substrings, a per-mutation timeout so a hang is a KILL rather than an
outage that leaves the mutant on disk, and a restore verified by BYTES.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
WN = os.path.join(ROOT, "crates/rewo-app/src/witness_names.rs")
BORDER = os.path.join(ROOT, "crates/rewo-app/src/bordershot_cmd.rs")

MUTATIONS = [
    ("control: no change", WN,
     "    const MIN_FILES: usize = 25;", "    const MIN_FILES: usize = 25;",
     "MUST SURVIVE — otherwise every verdict below is vacuous"),

    # ---- the claim itself, mutated in the DATA ---------------------------
    ("a gate file gains a real duplicate witness name",
     BORDER,
     '        "s2.the_extent_collapses_to_static_on_the_last_tick_not_after_it",',
     '        "s1.the_lerp_walks_one_tick_at_a_time_from_the_source_to_the_target",',
     "THE claim: two rows under one name, counted twice, one of them ungraded, "
     "and the gate still fail-closed on a count that now adds up. This is the "
     "only mutation here that can reach the headline assertion, because on a "
     "clean tree the assertion is unobservable from the check's own side"),

    # ---- the allow-list, in both directions ------------------------------
    ("an exclusion is widened past what the file needs",
     WN, '    ("soundshot_cmd", 4),', '    ("soundshot_cmd", 5),',
     "an allow-list that outlives its reason is how every allow-list fails; "
     "the second test exists to make the count EXACT rather than a ceiling"),
    ("an exclusion is narrower than the file needs",
     WN, '    ("soundshot_cmd", 4),', '    ("soundshot_cmd", 3),',
     "the other direction — the four `soundshot` rows really do have two "
     "definitions each, one per build configuration"),
    ("an exclusion is dropped entirely",
     WN, '    ("titleshot_cmd", 2),', "",
     "a removed entry must fail loudly rather than silently re-flagging a "
     "known-good file"),

    # ---- the instrument --------------------------------------------------
    ("the scanner finds nothing",
     WN, "        out.push(body[..end].to_string());", "        let _ = end;",
     "M138a: a check that turns a missing input into an empty result is green "
     "exactly on the machine where it stopped working. The floors are what "
     "turn that into a failure"),
    ("the scanner swallows the non-literal case",
     WN,
     "            let Some(body) = head.strip_prefix('\"') else {",
     "            let Some(body) = Some(head) else {",
     "a formatted name would be read as a literal starting at `format!(`, "
     "which the scanner's own witness pins"),

    # ---- the floors ------------------------------------------------------
    ("the file floor is removed",
     WN, "    const MIN_FILES: usize = 25;", "    const MIN_FILES: usize = 0;",
     "EXPECTED SURVIVOR, and it is the honest one: on a tree where the scanner "
     "works the floor is inert, which is exactly why m6 mutates the scanner "
     "instead. Kept so a future reader sees the pair"),
]


def run():
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", "rewo-app", "--bins", "witness_names"],
            cwd=ROOT, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=600,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    if p.returncode != 0:
        why = ("build failed"
               if "error[E" in p.stderr or "could not compile" in p.stderr
               else "tests failed")
        return False, why
    if "test result: ok" not in p.stdout:
        return False, "no test result line"
    return True, "passed"


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m167] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control")

    results = []
    for name, path, find, repl, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl, 1)
            )
            survived, reason = run()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
        results.append((name, verdict, why))

    print()
    if not results or results[0][1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        return 2
    killed = sum(1 for _, v, _ in results[1:] if v == "KILLED")
    total = len(results) - 1
    print(f"control SURVIVED (battery is valid) - {killed}/{total} killed")
    for name, verdict, why in results[1:]:
        if verdict != "KILLED":
            print(f"  {verdict}: {name}\n    would mean: {why}")
    # One expected, documented survivor: the file floor (see its `why`).
    return 0 if killed >= total - 1 else 1


if __name__ == "__main__":
    sys.exit(main())
