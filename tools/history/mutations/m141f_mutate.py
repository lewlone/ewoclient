"""M141f's mutation battery — the bee's anger, and postAddEntitySoundInstance.

    python tools/m141f_mutate.py

Same rules as its predecessors: verdicts from the TEST RESULT LINE rather than
the exit code, a NO-OP CONTROL that must SURVIVE, restore in a `finally`, a
per-run timeout, and a byte comparison at the end.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
M = os.path.join("crates", "rewo-net", "src", "metadata.rs")
L = os.path.join("crates", "rewo-net", "src", "lib.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
W = os.path.join("crates", "rewo-world", "src", "entities.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")

MUTATIONS = [
    (
        M,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "    /// Index 19 LONG — `Bee.DATA_ANGER_END_TIME` (M141f). Kind-gated by the",
        "    /// Index 19 LONG — `Bee.DATA_ANGER_END_TIME` (M141f). Gated by the",
        "SURVIVED",
    ),
    # --- the index and its serializer -------------------------------------
    (
        M,
        "the anger deadline is read at index 18 (AgeableMob counted as one)",
        "            (19, 2) => meta.long19 = r.varlong().ok(),",
        "            (18, 2) => meta.long19 = r.varlong().ok(),",
        "KILLED",
    ),
    (
        M,
        "LONG is read as a fixed i64 rather than a VAR_LONG",
        "            (19, 2) => meta.long19 = r.varlong().ok(),",
        "            (19, 2) => meta.long19 = r.i64().ok(),",
        "KILLED",
    ),
    # --- the kind gate ----------------------------------------------------
    (
        L,
        "the anger deadline is stored for any entity, not just a bee",
        "        if Some(type_id) == kinds.bee {\n            entities.set_anger_end_time(eid, t);\n        }",
        "        entities.set_anger_end_time(eid, t);",
        "KILLED",
    ),
    # --- the predicate ----------------------------------------------------
    (
        E,
        "anger is a stored flag rather than a deadline against the clock",
        "        crate::tickable::is_angry(\n            self.table.anger_end_time(entity_id).unwrap_or(-1),\n            self.game_time,\n        )",
        "        self.table.anger_end_time(entity_id).is_some()",
        "KILLED",
    ),
    (
        E,
        "an entity that never sent one defaults to angry",
        "            self.table.anger_end_time(entity_id).unwrap_or(-1),",
        "            self.table.anger_end_time(entity_id).unwrap_or(i64::MAX),",
        "KILLED",
    ),
    # --- the instances ----------------------------------------------------
    (
        E,
        "the minecart loop cannot start silent (a still cart never sounds)",
        "                volume: 0.0,\n                looping: true,\n                delay: 0,\n                can_start_silent: true,",
        "                volume: 0.0,\n                looping: true,\n                delay: 0,\n                can_start_silent: false,",
        "KILLED",
    ),
    (
        E,
        "the minecart is not silence-gated (it does override canPlaySound)",
        "                binding: Binding::Entity(minecart),",
        "                binding: Binding::Fixed,",
        "KILLED",
    ),
    (
        E,
        "the bee's spawn choice ignores its anger",
        "            let kind = if aggressive {\n                crate::tickable::BeeLoop::Aggressive\n            } else {\n                crate::tickable::BeeLoop::Flying\n            };",
        "            let _ = aggressive;\n            let kind = crate::tickable::BeeLoop::Flying;",
        "KILLED",
    ),
    # --- the table --------------------------------------------------------
    (
        W,
        "the anger deadline is not stored",
        "        self.anger_end_time.insert(id, end_time);",
        "        let _ = (id, end_time);",
        "KILLED",
    ),
    # --- the trigger, which is a composition root -------------------------
    (
        # `PlaySession` owns a socket and has no test module anywhere in the
        # repo, so its call sites survive by construction. Named rather than
        # hidden, exactly as M141d's and M141e's are; `--render-check` is the
        # instrument this class wants (M138a's r45).
        P,
        "COMPOSITION ROOT: the spawn trigger is never called (must SURVIVE)",
        "                self.post_add_entity_sound_instance(eid, type_id);",
        "                let _ = (eid, type_id);",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the bee arm is dropped (must SURVIVE)",
        "        } else if Some(type_id) == self.bee_type_id {",
        "        } else if false {",
        "SURVIVED",
    ),
]


def run_tests():
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note."""
    for attempt in range(2):
        outs, rcs = [], []
        for args in (
            ["cargo", "test", "-p", "rewo-world", "--lib"],
            ["cargo", "test", "-p", "rewo-net", "--lib"],
        ):
            try:
                p = subprocess.run(args, cwd=ROOT, capture_output=True, timeout=300)
            except subprocess.TimeoutExpired:
                for exe in ("rewo_world-*.exe", "rewo_net-*.exe"):
                    subprocess.run(["taskkill", "/F", "/IM", exe], capture_output=True)
                return "failed"
            outs.append((p.stdout + p.stderr).decode("utf-8", "replace"))
            rcs.append(p.returncode)
        joined = "\n".join(outs)
        if "test result: FAILED" in joined:
            return "failed"
        if all("test result: ok" in o for o in outs) and all(r == 0 for r in rcs):
            return "ok"
        if attempt == 0:
            time.sleep(3)
            continue
        sys.stderr.write(joined[-2000:] + "\n")
        return "build"
    return "build"


def main():
    paths = sorted({m[0] for m in MUTATIONS})
    snapshots = {p: io.open(os.path.join(ROOT, p), "rb").read() for p in paths}

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != "ok":
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for rel, name, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        text = snapshot.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-60s ANCHOR MATCHED %d TIMES" % (name[:60], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            r = run_tests()
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-60s %-10s (want %-9s) %s"
            % (name[:60], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

    leftover = [
        p for p in paths if io.open(os.path.join(ROOT, p), "rb").read() != snapshots[p]
    ]
    print("-----")
    print(
        "files restored: %s"
        % ("no -- MUTATION LEFT ON DISK: %s" % leftover if leftover else "yes")
    )
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
