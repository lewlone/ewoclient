"""M138d's mutation battery — the command ring's discipline.

    python tools/m138d_mutate.py

Memory-ordering mutations are deliberately NOT in here. Relaxing a `Release` to
a `Relaxed` is a real bug that a test can only catch by luck, on one processor's
memory model, on some runs — a battery entry that flakes is worse than none,
because a red run stops meaning anything. The orderings are argued in the type's
own doc instead, which is the honest place for a claim no test can hold.

Same rules as its predecessors: verdicts from the EXIT CODE, a NO-OP CONTROL
that must SURVIVE, restore in a `finally`, and compare the file's BYTES at the
end rather than asking `git diff --quiet`.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
D = os.path.join("crates", "rewo-audio", "src", "device.rs")

MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// A bounded single-producer / single-consumer ring.",
        "/// A bounded single-producer, single-consumer ring.",
        "SURVIVED",
    ),
    (
        "full test uses tail instead of the NEXT slot (off by one)",
        "        if next == self.head.load(Ordering::Acquire) {",
        "        if tail == self.head.load(Ordering::Acquire) {",
        "KILLED",
    ),
    (
        "overwrite the oldest on full instead of refusing the newest",
        "            self.dropped.fetch_add(1, Ordering::Relaxed);\n            return false;",
        "            self.head.store((self.head.load(Ordering::Acquire) + 1) % self.slots.len(), Ordering::Release);",
        "KILLED",
    ),
    (
        "stop counting drops",
        "            self.dropped.fetch_add(1, Ordering::Relaxed);",
        "            self.dropped.fetch_add(0, Ordering::Relaxed);",
        "KILLED",
    ),
    (
        "reset the drop count once there is room again",
        "        unsafe { *self.slots[tail].get() = Some(cmd) };",
        "        self.dropped.store(0, Ordering::Relaxed);\n        unsafe { *self.slots[tail].get() = Some(cmd) };",
        "KILLED",
    ),
    (
        "report the allocation as the capacity",
        "        self.slots.len() - 1",
        "        self.slots.len()",
        "KILLED",
    ),
    (
        "pop does not advance head",
        "        self.head\n            .store((head + 1) % self.slots.len(), Ordering::Release);",
        "        self.head.store(head, Ordering::Release);",
        "KILLED",
    ),
    (
        "indices grow without wrapping",
        "        let next = (tail + 1) % self.slots.len();",
        "        let next = tail + 1;",
        "KILLED",
    ),
]


def reap():
    """Kill any surviving test binary.

    A mutant that hangs leaves its exe running, and on Windows that exe holds
    the link output — so the NEXT run fails with linker error 1104 and reports
    a build failure for a mutation that was fine. Learned the hard way: the
    off-by-one full test makes the ring read FULL at construction (head == tail
    == 0), so the two-thread witness's producer spins forever."""
    subprocess.run(
        ["taskkill", "/F", "/IM", "rewo_audio-*.exe"],
        capture_output=True,
        shell=False,
    )


def run_tests():
    """A HANG IS A KILL, not an outage.

    Without a per-run timeout one spinning mutant takes the whole battery down
    with it, the `finally` never runs, and the mutation is left on disk — which
    is exactly what happened on this battery's first run. 120 s is roughly
    fifteen times the honest build-and-run, so it cannot misfire on a slow
    machine."""
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", "rewo-audio", "--lib"],
            cwd=ROOT,
            capture_output=True,
            timeout=120,
        )
        return p.returncode
    except subprocess.TimeoutExpired:
        reap()
        return 1  # a mutant that hangs the suite is caught by it


def main():
    path = os.path.join(ROOT, D)
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
            print("%-54s ANCHOR MATCHED %d TIMES" % (name[:54], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            verdict = "KILLED" if run_tests() != 0 else "SURVIVED"
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print("%-54s %-9s (want %-9s) %s" % (name[:54], verdict, want, "ok" if ok else "<<< UNEXPECTED"))

    reap()
    leftover = io.open(path, "rb").read() != snapshot
    print("-----")
    print("file restored: %s" % ("no -- MUTATION LEFT ON DISK" if leftover else "yes"))
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
