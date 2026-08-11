"""M141h's mutation battery — the riding pair.

    python tools/m141h_mutate.py

Same rules as its predecessors, and **run it alone** — see `m141g_mutate.py`'s
note on the interrupted-battery hazard.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
S = os.path.join("crates", "rewo-net", "src", "sounds.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
W = os.path.join("crates", "rewo-world", "src", "lib.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")

MUTATIONS = [
    (
        S,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `startRiding`'s minecart arm — **two instances**, and both are played.",
        "/// `startRiding`'s minecart arm — two instances, and both are played.",
        "SURVIVED",
    ),
    # --- the four constants -----------------------------------------------
    # NOT a length change: `let [wet, dry] = RIDING_MINECART` pins the arity at
    # COMPILE time, so shortening the array is a build failure rather than a
    # behavioural kill. The runtime claim is that the pair names two DIFFERENT
    # sounds, so that is what this mutates.
    (
        S,
        "the minecart pair names one sound twice",
        "        sound: \"minecraft:entity.minecart.inside.underwater\",",
        "        sound: \"minecraft:entity.minecart.inside\",",
        "KILLED",
    ),
    (
        S,
        "both minecart loops are the same side of the waterline",
        "        underwater_sound: true,\n        sound: \"minecraft:entity.minecart.inside.underwater\",",
        "        underwater_sound: false,\n        sound: \"minecraft:entity.minecart.inside.underwater\",",
        "KILLED",
    ),
    (
        S,
        "the nautilus takes the ghast's side of the waterline",
        "pub const RIDING_NAUTILUS: RidingSpec = RidingSpec {\n    player: 0,\n    vehicle: 0,\n    underwater_sound: true,",
        "pub const RIDING_NAUTILUS: RidingSpec = RidingSpec {\n    player: 0,\n    vehicle: 0,\n    underwater_sound: false,",
        "KILLED",
    ),
    (
        S,
        "the ghast takes the nautilus's side",
        "pub const RIDING_HAPPY_GHAST: RidingSpec = RidingSpec {\n    player: 0,\n    vehicle: 0,\n    underwater_sound: false,",
        "pub const RIDING_HAPPY_GHAST: RidingSpec = RidingSpec {\n    player: 0,\n    vehicle: 0,\n    underwater_sound: true,",
        "KILLED",
    ),
    (
        S,
        "the minecart's volume ceiling is the ghast's",
        "        volume_min: 0.0,\n        volume_max: 0.75,\n        volume_amplifier: 1.0,\n        is_minecart: true,\n    },\n    RidingSpec {",
        "        volume_min: 0.0,\n        volume_max: 1.0,\n        volume_amplifier: 1.0,\n        is_minecart: true,\n    },\n    RidingSpec {",
        "KILLED",
    ),
    (
        S,
        "a minecart rider uses the base class's hooks",
        "        is_minecart: true,\n    },\n];",
        "        is_minecart: false,\n    },\n];",
        "KILLED",
    ),
    # --- the instance ------------------------------------------------------
    (
        E,
        "the riding loop is attenuated",
        "                delay: 0,\n                can_start_silent: true,\n                attenuation: Attenuation::None,",
        "                delay: 0,\n                can_start_silent: true,",
        "KILLED",
    ),
    (
        E,
        "the riding loop cannot start silent (volume_min is 0)",
        "                volume: r.volume_min,\n                looping: true,\n                delay: 0,\n                can_start_silent: true,",
        "                volume: r.volume_min,\n                looping: true,\n                delay: 0,\n                can_start_silent: false,",
        "KILLED",
    ),
    (
        E,
        "the riding loop is gated on the rider rather than the vehicle",
        "                binding: Binding::Entity(r.vehicle),",
        "                binding: Binding::Entity(r.player),",
        "KILLED",
    ),
    # --- the underwater input ----------------------------------------------
    (
        E,
        "the local player's submersion is not consulted",
        "        self.local.is_some_and(|l| l.id == entity_id && l.underwater)",
        "        let _ = entity_id;\n        false",
        "KILLED",
    ),
    (
        E,
        "every entity reads the local player's submersion",
        "        self.local.is_some_and(|l| l.id == entity_id && l.underwater)",
        "        self.local.is_some_and(|l| l.underwater)",
        "KILLED",
    ),
    (
        W,
        "the water point query ignores the block it is given",
        "        let state = self.block_state_at(bx, by, bz) as usize;\n        water.get(state).copied().unwrap_or(false)",
        "        let _ = (bx, by, bz);\n        let _ = water;\n        false",
        "KILLED",
    ),
    # --- the trigger, a composition root -----------------------------------
    (
        P,
        "COMPOSITION ROOT: the mount trigger is never called (must SURVIVE)",
        "                self.start_riding_sound();",
        "",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the ghast arm is dropped (must SURVIVE)",
        "        } else if Some(type_id) == self.happy_ghast_type_id {",
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
