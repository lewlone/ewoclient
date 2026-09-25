"""M138a's mutation battery — DATA_SILENT, and whether anything reads it.

    python tools/m138a_mutate.py

The interesting mutation is the last one. Decoding a flag into a field nothing
consults is exactly the state this milestone found — index 4 was parsed from M1
and discarded, and `entity_silent` returned a hardcoded `false` — so a battery
that only mutates the DECODE would go green against a re-broken consumer.

Rules inherited from `tools/m135_mutate.py`: verdicts come from the EXIT CODE
and never from a substring; every battery carries a NO-OP CONTROL that must
SURVIVE, without which a run against an already-red tree reads KILLED for
everything; the file is restored in a `finally` and the tree is asserted clean
at the end, because a battery killed at the tool's 10-minute cap leaves its
mutation on disk.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
META = os.path.join("crates", "rewo-net", "src", "metadata.rs")
ENTS = os.path.join("crates", "rewo-world", "src", "entities.rs")
SOUND = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")

MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        META,
        "            // BOOLEAN at 4 = `Entity.DATA_SILENT` (M138a). Declared on the line",
        "            // BOOLEAN at 4 is `Entity.DATA_SILENT` (M138a). Declared on the line",
        "SURVIVED",
    ),
    (
        "drop the decode entirely",
        META,
        "            (4, 8) => meta.silent = r.bool().ok(),",
        "            (4, 8) => {}",
        "KILLED",
    ),
    (
        "read the flag off index 5 instead of 4",
        META,
        "            (4, 8) => meta.silent = r.bool().ok(),",
        "            (5, 8) => meta.silent = r.bool().ok(),",
        "KILLED",
    ),
    (
        "invert set_silent, so true mutes nothing",
        ENTS,
        "    pub fn set_silent(&mut self, id: i32, silent: bool) {\n        if silent {",
        "    pub fn set_silent(&mut self, id: i32, silent: bool) {\n        if !silent {",
        "KILLED",
    ),
    (
        "leave the flag behind when the entity is removed",
        ENTS,
        "        self.silent.remove(&id);\n        self.clear_riding(id);",
        "        self.clear_riding(id);",
        "KILLED",
    ),
    (
        "revert the CONSUMER to a hardcoded false (the pre-M138a state)",
        SOUND,
        "        self.0.is_silent(entity_id)",
        "        let _ = entity_id;\n        false",
        "KILLED",
    ),
    # --- the listener seam (items 1 and 2) --------------------------------
    (
        "flip the sign of forward.z",
        SOUND,
        "    ([-cp * sy, -sp, cp * cy], [-sp * sy, cp, sp * cy])",
        "    ([-cp * sy, -sp, -cp * cy], [-sp * sy, cp, sp * cy])",
        "KILLED",
    ),
    (
        "pin up to the constant (0, 1, 0) -- right at the horizon, wrong off it",
        SOUND,
        "    ([-cp * sy, -sp, cp * cy], [-sp * sy, cp, sp * cy])",
        "    ([-cp * sy, -sp, cp * cy], [0.0, 1.0, 0.0])",
        "KILLED",
    ),
    (
        "swap right() to up x forward -- the stereo image mirrored",
        SOUND,
        "            f[1] * u[2] - f[2] * u[1],\n            f[2] * u[0] - f[0] * u[2],\n            f[0] * u[1] - f[1] * u[0],",
        "            u[1] * f[2] - u[2] * f[1],\n            u[2] * f[0] - u[0] * f[2],\n            u[0] * f[1] - u[1] * f[0],",
        "KILLED",
    ),
    (
        "RecordingDevice drops the listener instead of recording it",
        SOUND,
        "    fn set_listener(&mut self, transform: ListenerTransform) {\n        self.listener_history.push(transform);\n    }",
        "    fn set_listener(&mut self, _transform: ListenerTransform) {}",
        "KILLED",
    ),
    (
        "update_listener builds a transform and never sends it",
        SOUND,
        "        self.device.set_listener(ListenerTransform {\n            position,\n            forward,\n            up,\n        });",
        "        let _ = (position, forward, up);",
        "KILLED",
    ),
]


def run_tests():
    """rewo-net owns the decode and the sound world; rewo-world owns the table.
    Both, by exit code."""
    for crate in ("rewo-net", "rewo-world"):
        p = subprocess.run(
            ["cargo", "test", "-p", crate, "--lib"], cwd=ROOT, capture_output=True
        )
        if p.returncode != 0:
            return p.returncode
    return 0


def main():
    # **Snapshot the touched files, do not ask git.** `git diff --quiet` cannot
    # tell "a mutation is still on disk" from "this milestone's own work is
    # uncommitted", so on a dirty tree it reports a leftover on every run and
    # the warning stops meaning anything. Comparing bytes answers the actual
    # question. (`m135_mutate.py` had the same flaw; it only ever ran on a clean
    # tree, which is why it never fired.)
    snapshots = {rel: io.open(os.path.join(ROOT, rel), "rb").read()
                 for _, rel, _, _, _ in MUTATIONS}

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != 0:
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for name, rel, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        original = io.open(path, "rb").read()
        text = original.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-56s ANCHOR MATCHED %d TIMES" % (name[:56], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            verdict = "KILLED" if run_tests() != 0 else "SURVIVED"
        finally:
            io.open(path, "wb").write(original)
        ok = verdict == want
        bad += 0 if ok else 1
        print("%-56s %-9s (want %-9s) %s" % (name[:56], verdict, want, "ok" if ok else "<<< UNEXPECTED"))

    leftover = [rel for rel, b in snapshots.items()
                if io.open(os.path.join(ROOT, rel), "rb").read() != b]
    dirty = 1 if leftover else 0
    print("-----")
    print("files restored: %s" % ("yes" if not leftover else "NO -- MUTATED: %s" % leftover))
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or dirty != 0) else 0)


if __name__ == "__main__":
    main()
