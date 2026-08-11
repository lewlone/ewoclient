"""M141e's mutation battery — the local player's shared flags and the elytra.

    python tools/m141e_mutate.py

Same rules as `m141_mutate.py` and `m141d_mutate.py`: verdicts from the TEST
RESULT LINE rather than the exit code, a NO-OP CONTROL that must SURVIVE,
restore in a `finally`, a per-run timeout, and a byte comparison at the end.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
L = os.path.join("crates", "rewo-net", "src", "local_player_data.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
T = os.path.join("crates", "rewo-net", "src", "tickable.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")

MUTATIONS = [
    (
        L,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `Entity.FLAG_FALL_FLYING`.",
        "/// `Entity.FLAG_FALL_FLYING` bit index.",
        "SURVIVED",
    ),
    # --- the flag ---------------------------------------------------------
    (
        L,
        "the fall-flying flag is not bit 7",
        "pub const FLAG_FALL_FLYING: u8 = 7;",
        "pub const FLAG_FALL_FLYING: u8 = 6;",
        "KILLED",
    ),
    # --- the rising edge --------------------------------------------------
    (
        L,
        "the edge ignores wasFallFlying (fires on every packet)",
        "        start_elytra_sound: data.is_fall_flying() && !data.was_fall_flying,",
        "        start_elytra_sound: data.is_fall_flying(),",
        "KILLED",
    ),
    (
        L,
        "the edge is a change guard rather than the tick sample",
        "    data.shared_flags = flags;\n    LocalMetaOutcome {\n        flags_updated: true,",
        "    let changed = data.shared_flags != flags;\n    data.shared_flags = flags;\n    if !changed {\n        return LocalMetaOutcome {\n            flags_updated: true,\n            start_elytra_sound: false,\n        };\n    }\n    LocalMetaOutcome {\n        flags_updated: true,",
        "KILLED",
    ),
    (
        L,
        "a packet without the flags entry still fires the edge",
        "    let Some(flags) = meta.flags else {",
        "    let Some(flags) = meta.flags.or(Some(data.shared_flags)) else {",
        "KILLED",
    ),
    (
        L,
        "the tick sample never runs (the edge never re-arms shut)",
        "        self.was_fall_flying = self.is_fall_flying();",
        "        self.was_fall_flying = false;",
        "KILLED",
    ),
    (
        L,
        "another entity's metadata is applied to the local player",
        "    if eid != player_id {\n        return LocalMetaOutcome::default();\n    }",
        "    if false {\n        return LocalMetaOutcome::default();\n    }",
        "KILLED",
    ),
    # --- the instance -----------------------------------------------------
    (
        E,
        "the elytra instance starts at volume 0 (play drops it as silent)",
        "                volume: 0.1,\n                looping: true,",
        "                volume: 0.0,\n                looping: true,",
        "KILLED",
    ),
    (
        E,
        "the elytra instance is not looping",
        "                volume: 0.1,\n                looping: true,",
        "                volume: 0.1,\n                looping: false,",
        "KILLED",
    ),
    (
        T,
        "the elytra becomes silence-gated like an EntityBoundSoundInstance",
        "            Ramp::Elytra(_)\n            | Ramp::UnderwaterLoop(_)",
        "            Ramp::Elytra(e) => Some(e.player),\n            Ramp::UnderwaterLoop(_)",
        "KILLED",
    ),
    (
        E,
        "the tick loop gates on the followed entity rather than the ramp's rule",
        "            if let Some(entity) = ramp.silence_gated_entity() {",
        "            if let Some(entity) = ramp.entity() {",
        "KILLED",
    ),
    (
        E,
        "a tickable for an unknown entity is played at the origin",
        "            let (x, y, z) = world.position(player).ok_or(NoInstance::UnknownEntity)?;",
        "            let (x, y, z) = world.position(player).unwrap_or((0.0, 0.0, 0.0));",
        "KILLED",
    ),
    # --- the queue path ---------------------------------------------------
    (
        E,
        "the queue path drops the ramp for a tickable",
        "                SoundEvent::Tickable(t) => instance_and_ramp(*t, world).map(|(i, r)| (i, Some(r))),",
        "                SoundEvent::Tickable(t) => instance_and_ramp(*t, world).map(|(i, _)| (i, None)),",
        "KILLED",
    ),
    (
        T,
        "an ordinary entity-bound sound loses its EntityBound ramp",
        "            crate::sound_instance::Binding::Entity(e) => Some(Ramp::EntityBound { entity: e }),",
        "            crate::sound_instance::Binding::Entity(_) => None,",
        "KILLED",
    ),
    (
        # Expected to SURVIVE for the same reason M141d's does: `PlaySession`
        # owns a socket and has no test module, so its call sites are
        # composition roots no unit test can reach. `--render-check` is the
        # instrument for this class (M138a's r45) and a witness for it is open.
        P,
        "COMPOSITION ROOT: the session stops capturing (must SURVIVE)",
        "            self.capture_local_metadata(body);",
        "",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the session stops sampling per tick (must SURVIVE)",
        "        self.local_player_data.tick();",
        "",
        "SURVIVED",
    ),
]


def run_tests():
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note."""
    for attempt in range(2):
        try:
            p = subprocess.run(
                ["cargo", "test", "-p", "rewo-net", "--lib"],
                cwd=ROOT,
                capture_output=True,
                timeout=300,
            )
        except subprocess.TimeoutExpired:
            subprocess.run(
                ["taskkill", "/F", "/IM", "rewo_net-*.exe"], capture_output=True
            )
            return "failed"
        out = (p.stdout + p.stderr).decode("utf-8", "replace")
        if "test result: FAILED" in out:
            return "failed"
        if "test result: ok" in out and p.returncode == 0:
            return "ok"
        if attempt == 0:
            time.sleep(3)
            continue
        sys.stderr.write(out[-2000:] + "\n")
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
