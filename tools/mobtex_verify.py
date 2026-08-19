"""Run the whole verification sweep for the mobtex milestone and report exit
codes rather than substrings.

Per-crate test runs, every serverless gate, and the demo PNG hash. `rewo-app`
is a BINARY crate and needs `--bins` where the other seven take `--lib`; a
crate whose tests fail to compile prints no `test result` line at all, so the
absence of one is reported as a failure rather than summed as zero.
"""

import hashlib
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")

CRATES = [
    ("rewo-proto", "--lib"),
    ("rewo-data", "--lib"),
    ("rewo-world", "--lib"),
    ("rewo-net", "--lib"),
    ("rewo-mesh", "--lib"),
    ("rewo-gpu", "--lib"),
    ("rewo-audio", "--lib"),
    ("rewo-app", "--bins"),
]

GATES = [
    "mobshot",
    "mobtexshot",
    "blockentityshot",
    "inventoryshot",
    "containershot",
    "swingshot",
    "itemshot",
    "capeshot",
    "titleshot",
    "hurtshot",
    "locatorshot",
    "labelshot",
    "statshot",
    "rideshot",
    "attributeshot",
    "hudshot",
    "deathshot",
    "serverlinkshot",
    "weathershot",
    "handshot",
    "particleshot",
    "healthbarshot",
    "bordershot",
    "eventshot",
    "danceshot",
    "breakshot",
    "captureshot",
    "portalshot",
    "abilityshot",
    "sidebarshot",
    "tablistshot",
    "soundshot",
    "skyshot",
    "lightmapshot",
    "tintshot",
    "meshshot",
    "dimensioncheck",
]


def tests():
    total = 0
    bad = []
    for name, flag in CRATES:
        r = subprocess.run(
            ["cargo", "test", "-p", name, flag],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=1800,
        )
        out = r.stdout + r.stderr
        lines = [l for l in out.splitlines() if l.startswith("test result:")]
        if not lines:
            bad.append(f"{name}: NO `test result` line (tests did not compile?)")
            print(f"{name:12} NO RESULT LINE  exit={r.returncode}")
            continue
        n = sum(int(l.split()[3]) for l in lines)
        f = sum(int(l.split()[5]) for l in lines)
        total += n
        if r.returncode != 0 or f:
            bad.append(f"{name}: exit={r.returncode} failed={f}")
        print(f"{name:12} {n:5} passed  {f} failed  exit={r.returncode}")
    print(f"TOTAL {total}")
    return bad


def enumerate_gates():
    """Take the gate list from `rewo --help`, not from the table above.

    §0.0's own advice — "enumerate them rather than trusting a list, since the
    list is what rots" — applied to this file. `GATES` is kept as a cross-check
    so a gate that *disappears* is as visible as one that appears.
    """
    r = subprocess.run([EXE, "--help"], cwd=ROOT, capture_output=True, text=True, timeout=120)
    found = []
    for line in (r.stdout + r.stderr).splitlines():
        w = line.strip().split()
        if w and (w[0].endswith("shot") or w[0].endswith("check")) and w[0].islower():
            found.append(w[0])
    missing = sorted(set(GATES) - set(found))
    extra = sorted(set(found) - set(GATES))
    if missing:
        print(f"!! in the hand list but not in --help: {missing}")
    if extra:
        print(f"!! in --help but not in the hand list: {extra}")
    return sorted(set(found) | set(GATES)), missing


def gates():
    found, missing = enumerate_gates()
    bad = [f"gate list drift: {missing}"] if missing else []
    for g in found:
        r = subprocess.run(
            [EXE, g, "--check"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=1800,
        )
        out = r.stdout + r.stderr
        vuid = out.count("VUID-")
        tail = [l for l in out.splitlines() if "witness" in l or "CHECK OK" in l]
        note = tail[-1].strip() if tail else ""
        ok = r.returncode == 0 and vuid == 0
        if not ok:
            bad.append(f"{g}: exit={r.returncode} vuids={vuid}")
        print(f"{g:20} exit={r.returncode} vuids={vuid}  {note[:90]}")
    return bad


def demo():
    out = os.path.join(os.environ.get("TEMP", "/tmp"), "rewo-demo-mobtex.png")
    r = subprocess.run(
        [EXE, "demo", "--out", out], cwd=ROOT, capture_output=True, text=True, timeout=900
    )
    if r.returncode != 0:
        print("demo FAILED", r.returncode)
        return ["demo exit != 0"]
    h = hashlib.sha256(open(out, "rb").read()).hexdigest()
    print(f"demo sha256 {h}")
    return [] if h.startswith("2cc56b4acbfb92cb") else [f"demo hash moved: {h}"]


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    bad = []
    if which in ("all", "tests"):
        bad += tests()
    if which in ("all", "gates"):
        bad += gates()
    if which in ("all", "demo"):
        bad += demo()
    print("\n--- failures ---" if bad else "\n--- all green ---")
    for b in bad:
        print(b)
    sys.exit(1 if bad else 0)
