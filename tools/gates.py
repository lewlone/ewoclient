#!/usr/bin/env python3
"""Run every serverless Rewo gate and report one table.

The gates are the `rewo <name> --check` subcommands (every `*shot`, plus
`dimensioncheck`). They are discovered from `rewo --help`, so a new gate is
picked up without editing this file, and a gate that stops being registered
disappears from the table instead of silently "passing".

Also renders `rewo demo` and compares its SHA-256 against
`tools/demo_hash.txt`. An intentional rendering change updates that file in the
same commit; nothing else should move it.

Exit status is non-zero if anything failed, timed out, or could not run.

Usage:
    python tools/gates.py                 # build (debug) + run everything
    python tools/gates.py --release       # use a release build
    python tools/gates.py --no-build      # use the existing binary
    python tools/gates.py --only mobshot,itemshot
    python tools/gates.py --audio         # build with --features audio
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
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEMO_HASH_FILE = ROOT / "tools" / "demo_hash.txt"
GATE_NAME = re.compile(r"^\s{2}([a-z0-9]+(?:shot|check))\b", re.MULTILINE)
VALIDATION = re.compile(r"VUID-|Validation Error")


def binary(release: bool) -> Path:
    exe = "rewo.exe" if os.name == "nt" else "rewo"
    return ROOT / "target" / ("release" if release else "debug") / exe


def build(release: bool, audio: bool) -> None:
    cmd = ["cargo", "build", "-p", "rewo-app"]
    if release:
        cmd.append("--release")
    if audio:
        cmd += ["--features", "audio"]
    print("$", " ".join(cmd), flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def discover(exe: Path) -> list[str]:
    out = subprocess.run([str(exe), "--help"], cwd=ROOT, capture_output=True,
                         text=True, encoding="utf-8", errors="replace", check=True)
    names = GATE_NAME.findall(out.stdout)
    if not names:
        sys.exit("no gates found in `rewo --help` — the help format changed")
    return names


def last_line(text: str) -> str:
    lines = [l for l in text.strip().splitlines() if l.strip()]
    return lines[-1][:120] if lines else ""


def run_gate(exe: Path, name: str, timeout: int) -> dict:
    t0 = time.monotonic()
    try:
        p = subprocess.run([str(exe), name, "--check"], cwd=ROOT, capture_output=True,
                           text=True, encoding="utf-8", errors="replace", timeout=timeout)
        text = p.stdout + "\n" + p.stderr
        # A gate can exit 0 while the validation layer reports errors (a leak at
        # device destroy, a sync hazard). Those are failures, not noise.
        vuids = len(VALIDATION.findall(text))
        if p.returncode != 0:
            status = f"FAIL({p.returncode})"
        elif vuids:
            status = "FAIL(vuid)"
        else:
            status = "PASS"
        detail = f"{vuids} validation error(s)" if vuids else last_line(text)
    except subprocess.TimeoutExpired:
        status, detail = "TIMEOUT", f"> {timeout}s"
    return {"gate": name, "status": status, "secs": round(time.monotonic() - t0, 1),
            "detail": detail}


def run_demo(exe: Path, timeout: int) -> dict:
    t0 = time.monotonic()
    expected = DEMO_HASH_FILE.read_text().strip() if DEMO_HASH_FILE.exists() else ""
    with tempfile.TemporaryDirectory() as d:
        out = Path(d) / "demo.png"
        try:
            p = subprocess.run([str(exe), "demo", "--out", str(out)], cwd=ROOT,
                               capture_output=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            return {"gate": "demo-hash", "status": "TIMEOUT", "secs": timeout, "detail": ""}
        if p.returncode != 0 or not out.exists():
            return {"gate": "demo-hash", "status": f"FAIL({p.returncode})",
                    "secs": round(time.monotonic() - t0, 1), "detail": "demo did not render"}
        got = hashlib.sha256(out.read_bytes()).hexdigest()
    ok = bool(expected) and got.startswith(expected)
    return {"gate": "demo-hash", "status": "PASS" if ok else "FAIL",
            "secs": round(time.monotonic() - t0, 1),
            "detail": f"sha256 {got[:16]} (expected {expected or '<missing>'})"}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--release", action="store_true")
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--audio", action="store_true", help="build with --features audio")
    ap.add_argument("--only", help="comma-separated gate names")
    ap.add_argument("--timeout", type=int, default=900, help="per-gate seconds")
    ap.add_argument("--no-demo", action="store_true")
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    if not args.no_build:
        build(args.release, args.audio)
    exe = binary(args.release)
    if not exe.exists():
        sys.exit(f"missing binary {exe} — build first or drop --no-build")

    names = discover(exe)
    if args.only:
        wanted = {n.strip() for n in args.only.split(",") if n.strip()}
        unknown = wanted - set(names)
        if unknown:
            sys.exit(f"unknown gate(s): {', '.join(sorted(unknown))}")
        names = [n for n in names if n in wanted]

    results = []
    for name in names:
        print(f"-- {name} ...", end=" ", flush=True)
        r = run_gate(exe, name, args.timeout)
        print(r["status"], f"({r['secs']}s)", flush=True)
        results.append(r)
    if not args.no_demo and not args.only:
        print("-- demo-hash ...", end=" ", flush=True)
        r = run_demo(exe, args.timeout)
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
