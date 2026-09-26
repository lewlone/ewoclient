"""M140's mutation battery — level_event's sounds.

    python tools/m140_mutate.py

Rules inherited from the M138 harnesses: verdicts from the EXIT CODE, a NO-OP
CONTROL that must SURVIVE, restore in a `finally`, a per-run timeout so a hang
is a KILL rather than an outage, and a byte comparison at the end rather than
`git diff --quiet`.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIB = os.path.join("crates", "rewo-net", "src", "lib.rs")

MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// The sound a `level_event` packet asks for (M140).",
        "/// The sound a `level_event` packet asks for. (M140)",
        "SURVIVED",
    ),
    (
        "the block CORNER instead of its centre",
        "        x: x as f64 + 0.5,\n        y: y as f64 + 0.5,\n        z: z as f64 + 0.5,",
        "        x: x as f64,\n        y: y as f64,\n        z: z as f64,",
        "KILLED",
    ),
    (
        "offset only x and z, leaving y at the corner",
        "        y: y as f64 + 0.5,",
        "        y: y as f64,",
        "KILLED",
    ),
    (
        "ignore the packet's global flag",
        "    let row = rewo_data::level_event_sounds::resolve(kind, data, global)?;",
        "    let row = rewo_data::level_event_sounds::resolve(kind, data, false)?;",
        "KILLED",
    ),
    (
        "ignore the data gate (every id takes its first row)",
        "    let row = rewo_data::level_event_sounds::resolve(kind, data, global)?;",
        "    let row = rewo_data::level_event_sounds::rows_for(kind).next()?;",
        "KILLED",
    ),
    (
        "assume volume 1.0 everywhere",
        "        volume: row.volume.unwrap_or(1.0),",
        "        volume: 1.0,",
        "KILLED",
    ),
    (
        "emit camera- and listener-placed rows at the block too",
        "    if row.placement != Placement::Block {\n        return None;\n    }",
        "",
        "KILLED",
    ),
    (
        "hand over a fixed range instead of letting getRange decide",
        "            fixed_range: None,",
        "            fixed_range: Some(16.0),",
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
    path = os.path.join(ROOT, LIB)
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
