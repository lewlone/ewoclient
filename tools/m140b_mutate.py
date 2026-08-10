"""M140b's mutation battery — MusicManager's gain ramp.

    python tools/m140b_mutate.py

Same rules as its predecessors: verdicts from the EXIT CODE, a NO-OP CONTROL
that must SURVIVE, restore in a `finally`, a per-run timeout so a hang is a KILL
rather than an outage, and a byte comparison at the end rather than
`git diff --quiet`.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
M = os.path.join("crates", "rewo-net", "src", "music.rs")

STOP = "            self.playing = false;\n            return false;"
STOP_MUT = "            self.gain = 0.0;\n            return true;"
DEFAULT = "            gain: 1.0,\n            playing: false,"
DEFAULT_MUT = "            gain: 0.0,\n            playing: false,"

MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `MusicManager`'s crossfade state.",
        "/// `MusicManager`'s cross-fade state.",
        "SURVIVED",
    ),
    (
        "a constant upward step instead of the gain itself",
        "            self.gain += self.gain.clamp(5.0e-4, 0.005);",
        "            self.gain += 0.005;",
        "KILLED",
    ),
    (
        "swap the upward clamp bounds",
        "            self.gain += self.gain.clamp(5.0e-4, 0.005);",
        "            self.gain += self.gain.clamp(0.005, 5.0e-4);",
        "KILLED",
    ),
    (
        "swap the blend weights on the way down",
        "            self.gain = 0.03 * volume + 0.97 * self.gain;",
        "            self.gain = 0.97 * volume + 0.03 * self.gain;",
        "KILLED",
    ),
    (
        "make the fall symmetric with the rise (a subtraction)",
        "            self.gain = 0.03 * volume + 0.97 * self.gain;",
        "            self.gain -= self.gain.clamp(5.0e-4, 0.005);",
        "KILLED",
    ),
    (
        # Expected to SURVIVE, and PROVEN so rather than assumed: the
        # else-branch is entered only when gain > volume, and the blend is
        # volume + 0.97*(gain - volume), which for a positive difference is
        # always > volume. The disjunct is dead code in vanilla too. See
        # `the_downward_overshoot_disjunct_cannot_fire`.
        "UNREACHABLE: the downward overshoot disjunct (must SURVIVE)",
        "            if (self.gain - volume).abs() < 1.0e-4 || self.gain < volume {",
        "            if (self.gain - volume).abs() < 1.0e-4 {",
        "SURVIVED",
    ),
    (
        "clamp at the floor instead of stopping the track",
        STOP,
        STOP_MUT,
        "KILLED",
    ),
    (
        "start silent instead of at full gain",
        DEFAULT,
        DEFAULT_MUT,
        "KILLED",
    ),
]


def run_tests():
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", "rewo-net", "--lib"],
            cwd=ROOT,
            capture_output=True,
            timeout=300,
        )
        return p.returncode
    except subprocess.TimeoutExpired:
        subprocess.run(["taskkill", "/F", "/IM", "rewo_net-*.exe"], capture_output=True)
        return 1


def main():
    path = os.path.join(ROOT, M)
    snapshot = io.open(path, "rb").read()

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != 0:
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for name, old, new, want in MUTATIONS:
        text = snapshot.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-52s ANCHOR MATCHED %d TIMES" % (name[:52], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            verdict = "KILLED" if run_tests() != 0 else "SURVIVED"
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print("%-52s %-9s (want %-9s) %s" % (name[:52], verdict, want, "ok" if ok else "<<< UNEXPECTED"))

    leftover = io.open(path, "rb").read() != snapshot
    print("-----")
    print("file restored: %s" % ("no -- MUTATION LEFT ON DISK" if leftover else "yes"))
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
