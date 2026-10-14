#!/usr/bin/env python3
"""Run every serverless Rewo gate and report one table.

The gates are the `rewo <name> --check` subcommands (every `*shot`, plus
`dimensioncheck`). They are discovered from `rewo --help` and then compared
against the pinned list `EXPECTED_GATES` below: a gate that stops being
registered, or a new one nobody added to the pin, FAILS the run (with the diff)
instead of silently shrinking or growing the sweep. Adding a gate means adding
its name to `EXPECTED_GATES` in the same commit.

Also runs:
  * the four `mobshot` sub-gates (`--emissive-check`, `--etf-check`,
    `--tint-check`, `--variant-check`), which are separate modes of one
    subcommand and so are invisible to the `--help` discovery;
  * `rewo demo`, comparing its SHA-256 against `tools/demo_hash.txt` and scanning
    its output for validation errors like every other row. An intentional
    rendering change updates that file in the same commit; nothing else should
    move it.

Every child runs with `RUST_LOG=info` (the binary's own default) and without
`REWO_NO_SYNC_VALIDATION`, so a caller's environment cannot silence the
validation callback or weaken it. A row passes only if it exits 0 AND its
output carries no `VUID-` / `Validation Error` line.

Validation is only guaranteed in a DEBUG build (`rewo demo`, `rewo bench` and the
windowed client gate it on `debug_assertions`; the gates assert it themselves and
fail if it is off). `--release` therefore prints a warning and the `demo-hash`
row can no longer vouch for validation. Use a debug build for the real sweep.

`--audio` builds with `--features gates,audio`, which makes `soundshot` grade the
decode and mixer layers as well (the default build grades only the wire, seeded
variant and device-call layers: "w+s+a"). The sweep is the same either way; the
row for `soundshot` additionally fails if an `--audio` run reports that it did
not grade the audio layers. CI runs the default sweep and then
`python tools/gates.py --audio --only soundshot`.

Exit status is non-zero if anything failed, timed out, or could not run.

Usage:
    python tools/gates.py                 # build (debug) + run everything
    python tools/gates.py --release       # use a release build (warns; see above)
    python tools/gates.py --no-build      # use the existing binary
    python tools/gates.py --only mobshot,itemshot   # `mobshot` includes its sub-gates
    python tools/gates.py --audio         # build with --features gates,audio
    python tools/gates.py --json out.json # machine-readable result

Most gates need the local 26.2 assets under %APPDATA%/EwoClient/rewo/26.2 and a
Vulkan device with validation layers; they cannot run on a machine without them.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time

from gpu_lock import machine_lock
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEMO_HASH_FILE = ROOT / "tools" / "demo_hash.txt"
GATE_NAME = re.compile(r"^\s{2}([a-z0-9]+(?:shot|check))\b", re.MULTILINE)
VALIDATION = re.compile(r"VUID-|Validation Error")

# The pinned gate set. `rewo --help` is the source of truth for what exists;
# this list is the check on it. Keep it sorted.
EXPECTED_GATES = [
    "abilityshot", "advshot", "attributeshot", "blockentityshot", "bookshot",
    "bordershot", "breakshot", "capeshot", "captureshot", "confirmlinkshot",
    "containershot", "danceshot", "deathshot", "dimensioncheck", "eventshot",
    "gaugeshot", "handshot", "healthbarshot", "hudshot", "hurtshot",
    "inventoryshot", "itemshot", "labelshot", "leashshot", "lightmapshot",
    "locatorshot", "meshshot", "mobshot", "mobtexshot", "optionshot",
    "particleshot", "portalshot", "rideshot", "serverlinkshot", "sidebarshot",
    "signshot", "skyshot", "soundshot", "statshot", "swingshot",
    "tablistshot", "tintshot", "titleshot", "weathershot",
]

# Extra rows: (row label, parent gate, argv after the binary). They are modes of
# a subcommand the discovery already lists, so they run by default and follow
# their parent under `--only`.
SUB_GATES = [
    ("mobshot:emissive", "mobshot", ["mobshot", "--emissive-check"]),
    ("mobshot:etf", "mobshot", ["mobshot", "--etf-check"]),
    ("mobshot:tint", "mobshot", ["mobshot", "--tint-check"]),
    ("mobshot:variant", "mobshot", ["mobshot", "--variant-check"]),
]


def binary(release: bool) -> Path:
    exe = "rewo.exe" if os.name == "nt" else "rewo"
    return ROOT / "target" / ("release" if release else "debug") / exe


def cargo_features(audio: bool) -> str:
    """The `--features` value: `gates`, plus `audio` when asked."""
    return ",".join(["gates"] + (["audio"] if audio else []))


def build(release: bool, audio: bool) -> None:
    cmd = ["cargo", "build", "-p", "rewo-app", "--features", cargo_features(audio)]
    if release:
        cmd.append("--release")
    print("$", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def child_env() -> dict:
    """The environment every child runs in.

    The validation callback logs through `log::error!`, so a caller's
    `RUST_LOG=off` (or `rewo_gpu=off`) would hide VUIDs from the scan below.
    Pin the binary's own default filter, and drop the one knob that weakens
    validation.
    """
    env = dict(os.environ)
    env["RUST_LOG"] = "info"
    env.pop("REWO_NO_SYNC_VALIDATION", None)
    return env


def discover(exe: Path) -> list[str]:
    out = subprocess.run([str(exe), "--help"], cwd=ROOT, capture_output=True,
                         text=True, encoding="utf-8", errors="replace", check=True,
                         env=child_env())
    names = GATE_NAME.findall(out.stdout)
    if not names:
        sys.exit("no gates found in `rewo --help` — the help format changed")
    return names


def check_pinned(names: list[str]) -> None:
    """Fail unless the discovered gate set is exactly `EXPECTED_GATES`."""
    found, pinned = set(names), set(EXPECTED_GATES)
    if len(names) != len(found):
        dup = sorted({n for n in names if names.count(n) > 1})
        sys.exit(f"gate set check FAILED: `rewo --help` lists duplicate gate(s): {', '.join(dup)}")
    if len(EXPECTED_GATES) != len(pinned):
        sys.exit("gate set check FAILED: EXPECTED_GATES in tools/gates.py has duplicates")
    if found == pinned:
        return
    lines = ["gate set check FAILED: `rewo --help` and EXPECTED_GATES disagree "
             f"({len(found)} discovered, {len(pinned)} pinned)"]
    for n in sorted(pinned - found):
        lines.append(f"  - missing  {n}: pinned but not registered (a gate was dropped or renamed)")
    for n in sorted(found - pinned):
        lines.append(f"  + unpinned {n}: registered but not in EXPECTED_GATES (add it in the same commit)")
    sys.exit("\n".join(lines))


def last_line(text: str) -> str:
    lines = [l for l in text.strip().splitlines() if l.strip()]
    return lines[-1][:120] if lines else ""


def grade(label: str, p: subprocess.CompletedProcess, t0: float, audio: bool, gate: str) -> dict:
    text = p.stdout + "\n" + p.stderr
    # A gate can exit 0 while the validation layer reports errors (a leak at
    # device destroy, a sync hazard). Those are failures, not noise.
    vuids = len(VALIDATION.findall(text))
    if p.returncode != 0:
        status = f"FAIL({p.returncode})"
    elif vuids:
        status = "FAIL(vuid)"
    elif audio and gate == "soundshot" and "DEFAULT build" in text:
        # `--audio` promises the decode and mixer layers were graded.
        status = "FAIL(audio)"
    else:
        status = "PASS"
    detail = f"{vuids} validation error(s)" if vuids else last_line(text)
    if status == "FAIL(audio)":
        detail = "--audio given but soundshot graded only the default layers"
    return {"gate": label, "status": status, "secs": round(time.monotonic() - t0, 1),
            "detail": detail}


def run_gate(exe: Path, label: str, argv: list[str], timeout: int, audio: bool,
             gate: str) -> dict:
    t0 = time.monotonic()
    try:
        p = subprocess.run([str(exe), *argv], cwd=ROOT, capture_output=True,
                           text=True, encoding="utf-8", errors="replace",
                           timeout=timeout, env=child_env())
    except subprocess.TimeoutExpired:
        return {"gate": label, "status": "TIMEOUT", "secs": round(time.monotonic() - t0, 1),
                "detail": f"> {timeout}s"}
    return grade(label, p, t0, audio, gate)


def run_demo(exe: Path, timeout: int, release: bool) -> dict:
    t0 = time.monotonic()
    expected = DEMO_HASH_FILE.read_text().strip() if DEMO_HASH_FILE.exists() else ""
    with tempfile.TemporaryDirectory() as d:
        out = Path(d) / "demo.png"
        try:
            p = subprocess.run([str(exe), "demo", "--out", str(out)], cwd=ROOT,
                               capture_output=True, text=True, encoding="utf-8",
                               errors="replace", timeout=timeout, env=child_env())
        except subprocess.TimeoutExpired:
            return {"gate": "demo-hash", "status": "TIMEOUT", "secs": timeout, "detail": ""}
        if p.returncode != 0 or not out.exists():
            return {"gate": "demo-hash", "status": f"FAIL({p.returncode})",
                    "secs": round(time.monotonic() - t0, 1), "detail": "demo did not render"}
        got = hashlib.sha256(out.read_bytes()).hexdigest()
    vuids = len(VALIDATION.findall(p.stdout + "\n" + p.stderr))
    ok = bool(expected) and got.startswith(expected)
    if vuids:
        status, detail = "FAIL(vuid)", f"{vuids} validation error(s)"
    elif ok:
        status = "PASS"
        detail = f"sha256 {got[:16]} (expected {expected})"
    else:
        status = "FAIL"
        detail = f"sha256 {got[:16]} (expected {expected or '<missing>'})"
    if release:
        detail += " [release: demo runs without validation]"
    return {"gate": "demo-hash", "status": status,
            "secs": round(time.monotonic() - t0, 1), "detail": detail}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--release", action="store_true",
                    help="use a release build (validation is not guaranteed; see the docstring)")
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--audio", action="store_true",
                    help="build with --features gates,audio so soundshot also grades decode + mixer")
    ap.add_argument("--only", help="comma-separated gate names (a gate pulls in its sub-gates)")
    ap.add_argument("--timeout", type=int, default=900, help="per-gate seconds")
    ap.add_argument("--no-demo", action="store_true")
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    if args.release:
        bar = "!" * 78
        print(bar, file=sys.stderr)
        print("WARNING: --release. Vulkan validation is debug-gated for `rewo demo`, so the "
              "demo-hash row cannot\nvouch for validation, and a release sweep is not the "
              "sweep CI runs. The gates themselves\nassert validation and fail if it is "
              "off, but use a debug build for a real verdict.", file=sys.stderr)
        print(bar, file=sys.stderr, flush=True)

    if not args.no_build:
        build(args.release, args.audio)
    exe = binary(args.release)
    if not exe.exists():
        sys.exit(f"missing binary {exe} — build first or drop --no-build")

    names = discover(exe)
    check_pinned(names)  # against the full set, before --only narrows it
    rows = [(n, [n, "--check"], n) for n in names]
    rows += [(label, argv, parent) for (label, parent, argv) in SUB_GATES if parent in names]
    if args.only:
        wanted = {n.strip() for n in args.only.split(",") if n.strip()}
        known = set(names) | {label for (label, _, _) in rows}
        unknown = wanted - known
        if unknown:
            sys.exit(f"unknown gate(s): {', '.join(sorted(unknown))}")
        rows = [r for r in rows if r[0] in wanted or r[2] in wanted]

    results = []
    # One GPU verification per machine: the self-hosted CI runner shares this
    # desktop and device with local runs (see tools/gpu_lock.py).
    with machine_lock("tools/gates.py"):
        for label, argv, parent in rows:
            print(f"-- {label} ...", end=" ", flush=True)
            r = run_gate(exe, label, argv, args.timeout, args.audio, parent)
            print(r["status"], f"({r['secs']}s)", flush=True)
            results.append(r)
        if not args.no_demo and not args.only:
            print("-- demo-hash ...", end=" ", flush=True)
            r = run_demo(exe, args.timeout, args.release)
            print(r["status"], flush=True)
            results.append(r)

    width = max(len(r["gate"]) for r in results)
    print()
    for r in results:
        print(f"{r['gate']:<{width}}  {r['status']:<10} {r['secs']:>7}s  {r['detail']}")
    failed = [r for r in results if r["status"] != "PASS"]
    print(f"\n{len(results) - len(failed)}/{len(results)} passed")
    if args.json:
        args.json.write_text(json.dumps(results, indent=2))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
