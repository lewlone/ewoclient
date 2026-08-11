"""M141d's mutation battery — the velocity input for the tickable ramps.

    python tools/m141d_mutate.py

Same rules as `m141_mutate.py`, including its correction: verdicts come from
the TEST RESULT LINE rather than the exit code, because reading the exit code
cannot tell a failing test from a failing BUILD and reported that battery's own
no-op control as KILLED.

Two crates, because the model and its seam are in different ones: the decay and
the deadband live in `rewo-world`'s `EntityState`, the queries that read them in
`rewo-net`'s `EntityTableWorld`. A mutation in one is graded by whichever
crate's tests can see it, so both are run for every entry.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
W = os.path.join("crates", "rewo-world", "src", "entities.rs")
N = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")
M = os.path.join("crates", "rewo-net", "src", "motion.rs")

MUTATIONS = [
    (
        W,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "    /// `entity.getDeltaMovement()`.",
        "    /// `entity.getDeltaMovement()` accessor.",
        "SURVIVED",
    ),
    # --- the decay --------------------------------------------------------
    (
        W,
        "the decay runs for a minecart too (aiStep is LivingEntity's)",
        "        if !self.motion_living {\n            // `aiStep` is `LivingEntity`'s. A minecart holds its last packet.\n            return;\n        }",
        "        if false {\n            return;\n        }",
        "KILLED",
    ),
    (
        W,
        "the decay factor is not 0.98",
        "                *c *= 0.98;",
        "                *c *= 0.9;",
        "KILLED",
    ),
    (
        W,
        "the decay runs BESIDE the lerp rather than opposite it",
        "        if !interpolating && !authoritative {",
        "        if !authoritative {",
        "KILLED",
    ),
    (
        W,
        "a locally-ridden vehicle decays like any other",
        "        if !interpolating && !authoritative {",
        "        if !interpolating {",
        "KILLED",
    ),
    # --- the deadband -----------------------------------------------------
    (
        W,
        "the deadband is per-axis for a player too",
        "        if self.motion_player {\n            if x * x + z * z < 9.0e-6 {\n                dx = 0.0;\n                dz = 0.0;\n            }\n        } else {",
        "        if false {\n            if x * x + z * z < 9.0e-6 {\n                dx = 0.0;\n                dz = 0.0;\n            }\n        } else {",
        "KILLED",
    ),
    (
        W,
        "the deadband is joint for a mob too",
        "            if x.abs() < 0.003 {\n                dx = 0.0;\n            }\n            if z.abs() < 0.003 {\n                dz = 0.0;\n            }",
        "            if x * x + z * z < 9.0e-6 {\n                dx = 0.0;\n                dz = 0.0;\n            }",
        "KILLED",
    ),
    (
        W,
        "the deadband threshold moved",
        "        if y.abs() < 0.003 {",
        "        if y.abs() < 0.03 {",
        "KILLED",
    ),
    (
        W,
        "the deadband is skipped entirely (the decay never lands on zero)",
        "        let [x, y, z] = self.delta_movement;",
        "        if true {\n            return;\n        }\n        let [x, y, z] = self.delta_movement;",
        "KILLED",
    ),
    # --- lerpMotion -------------------------------------------------------
    (
        W,
        "a non-finite movement is stored rather than ignored",
        "        if movement.iter().all(|c| c.is_finite()) {\n            e.delta_movement = movement;\n        }",
        "        e.delta_movement = movement;",
        "KILLED",
    ),
    (
        W,
        "lerpMotion accumulates rather than replacing",
        "            e.delta_movement = movement;",
        "            for (a, b) in e.delta_movement.iter_mut().zip(movement) {\n                *a += b;\n            }",
        "KILLED",
    ),
    (
        W,
        "the controlling passenger is any passenger, not the first",
        "                    .and_then(|riders| riders.first())\n                    .copied()\n                    == Some(p)",
        "                    .map(|riders| riders.contains(&p))\n                    == Some(true)",
        "KILLED",
    ),
    # --- the reductions ---------------------------------------------------
    (
        W,
        "horizontalDistance includes the Y term",
        "            .map_or(0.0, |v| (v[0] * v[0] + v[2] * v[2]).sqrt())",
        "            .map_or(0.0, |v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())",
        "KILLED",
    ),
    (
        W,
        "lengthSqr is rooted",
        "            .map_or(0.0, |v| v[0] * v[0] + v[1] * v[1] + v[2] * v[2])",
        "            .map_or(0.0, |v| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt())",
        "KILLED",
    ),
    # --- the seam ---------------------------------------------------------
    (
        N,
        "the ramp world ignores the table's velocity",
        "        self.table.horizontal_speed(entity_id)\n    }",
        "        let _ = entity_id;\n        0.0\n    }",
        "KILLED",
    ),
    (
        N,
        "the local player's velocity is not consulted",
        "        if let Some(l) = self.local.filter(|l| l.id == entity_id) {\n            let (x, _, z) = l.velocity;\n            return (x * x + z * z).sqrt();\n        }",
        "",
        "KILLED",
    ),
    (
        N,
        "the local player's position is not consulted (reads as removed)",
        "        if let Some(l) = self.local.filter(|l| l.id == entity_id) {\n            // Not in the table, and `None` here is `isRemoved()` — so without\n            // this branch the elytra ramp stops itself on its first tick.\n            return Some(l.position);\n        }",
        "",
        "KILLED",
    ),
    (
        N,
        "the local view answers for EVERY id, not just the local one",
        "        if let Some(l) = self.local.filter(|l| l.id == entity_id) {\n            let (x, y, z) = l.velocity;\n            return x * x + y * y + z * z;\n        }",
        "        if let Some(l) = self.local {\n            let (x, y, z) = l.velocity;\n            return x * x + y * y + z * z;\n        }",
        "KILLED",
    ),
    (
        M,
        "a remote entity's motion packet is dropped again",
        "    entities.lerp_motion(\n        m.id,\n        [m.movement.x, m.movement.y, m.movement.z],\n        living,\n        player,\n    );",
        "    let _ = (living, player);\n    let _ = (entities, m);",
        "KILLED",
    ),
    (
        M,
        "the class facts are ignored, so nothing ever decays",
        "        Some(t) => (living(t), player(t)),",
        "        Some(_) => (false, false),",
        "KILLED",
    ),
    (
        # Expected to SURVIVE, and recorded rather than papered over.
        # `PlaySession` owns a socket and has no test module anywhere in the
        # repo, so its call sites are composition roots that no unit test can
        # reach — the same class `REWO_AUDIO_PLAN.md` §0.3 records for
        # `LiveSounds::drive`, which needed `--render-check`'s r45 to cover.
        # This one wants a live server sending motion for a remote MOB, which
        # is a `--render-check` witness of its own and is named in §15 as open.
        P,
        "COMPOSITION ROOT: the session stops calling it (must SURVIVE)",
        "            crate::motion::apply_remote_motion(\n                &mut self.world.entities,\n                self.entity_classes.as_deref(),\n                m,\n            );\n            return;",
        "            return;",
        "SURVIVED",
    ),
]


def run_tests():
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note."""
    for attempt in range(2):
        outs = []
        rcs = []
        for args in (
            ["cargo", "test", "-p", "rewo-world", "--lib"],
            ["cargo", "test", "-p", "rewo-net", "--lib"],
        ):
            try:
                p = subprocess.run(args, cwd=ROOT, capture_output=True, timeout=300)
            except subprocess.TimeoutExpired:
                subprocess.run(
                    ["taskkill", "/F", "/IM", "rewo_world-*.exe"], capture_output=True
                )
                subprocess.run(
                    ["taskkill", "/F", "/IM", "rewo_net-*.exe"], capture_output=True
                )
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
            print("%-58s ANCHOR MATCHED %d TIMES" % (name[:58], n))
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
            "%-58s %-10s (want %-9s) %s"
            % (name[:58], verdict, want, "ok" if ok else "<<< UNEXPECTED")
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
