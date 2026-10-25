"""Rewo vs vanilla playtest: play tools/playtest/*.yaml with vanilla 26.2 and with Rewo, then compare.

Runs through the shared playtest harness (Desktop/playtest-harness, or PLAYTEST_HARNESS): vanilla is
the harness's puppet client, Rewo is this checkout's `rewo live` driven by REWO_SCRIPT
(crates/rewo-app/src/live_cmd/script.rs). Both join the same fresh vanilla server; the harness then
compares every capture (SSIM per region), every sound (id, volume, pitch, position, resolved
variant), the position trace and the server corrections, and with --review asks Opus about the
flagged frames (about $0.25 per scenario).

    python tools/playtest.py                      # every scenario
    python tools/playtest.py rewo_hud rewo_boat   # some
    python tools/playtest.py --review             # plus the Opus review
    python tools/playtest.py --no-build           # reuse target/debug/rewo.exe
    python tools/playtest.py --list

A difference is a finding, not a failure: renderers differ. The exit code is non-zero only when a
run itself failed (a client crashed, a scenario timed out). Results: one compare directory per
scenario under the harness's runs/, and a summary (JSON + Markdown) printed and written to
<harness>/runs/playtest-<stamp>.json.

Shared-PC rules hold: the harness takes tools/gpu_lock.py around each client run, waits while
G:\\w\\specs\\G4-RUNNING exists (PLAYTEST_WAIT_G4, default an hour), and its server listens on
PLAYTEST_PORT (default 25712 here, inside the harness's 25710-25719).
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCENARIOS = ROOT / "tools" / "playtest"
HARNESS = Path(os.environ.get("PLAYTEST_HARNESS", Path.home() / "Desktop" / "playtest-harness"))


def build() -> Path:
    print("[playtest] cargo build -p rewo-app --features gates", flush=True)
    r = subprocess.run(["cargo", "build", "-p", "rewo-app", "--features", "gates"], cwd=ROOT)
    if r.returncode != 0:
        raise SystemExit("build failed")
    return ROOT / "target" / "debug" / "rewo.exe"


def summarise(name: str, cmp: dict, review: dict | None) -> dict:
    frames = cmp["frames"]
    worst = {}
    for f in frames:
        for region, v in (f.get("ssim") or {}).items():
            worst[region] = min(worst.get(region, 1.0), v)
    s = cmp["sounds"]
    out = {
        "scenario": name,
        "frames": len(frames),
        "flagged_frames": cmp["flagged"]["frames"],
        "min_ssim": {k: round(v, 4) for k, v in sorted(worst.items())},
        "sounds": {"reference": s["reference_count"], "rewo": s["candidate_count"], "matched": s["matched"],
                   "missing": len(s["missing"]), "extra": len(s["extra"]), "different": len(s["different"])},
        "corrections": cmp["corrections"],
        "position_max_deviation": cmp["positions"]["max_deviation"],
        "expectations": cmp["expectations"],
    }
    if review:
        out["review"] = {"overall": review.get("overall"), "cost_usd": review.get("cost_usd"),
                         "findings": [{k: f.get(k) for k in ("frame", "severity", "summary")}
                                      for f in review.get("findings", [])]}
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("names", nargs="*", help="scenario names (default: all in tools/playtest)")
    ap.add_argument("--review", action="store_true", help="Opus review of flagged frames (~$0.25 each)")
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--rewo-only", action="store_true", help="run Rewo only (no vanilla, no compare)")
    a = ap.parse_args(argv)

    all_scen = sorted(SCENARIOS.glob("*.yaml"))
    if a.list:
        for p in all_scen:
            print(p.stem)
        return 0
    chosen = [p for p in all_scen if not a.names or p.stem in a.names]
    missing = set(a.names) - {p.stem for p in chosen}
    if missing:
        print(f"unknown scenarios: {sorted(missing)}", file=sys.stderr)
        return 2
    if not HARNESS.exists():
        print(f"playtest harness not found at {HARNESS} (set PLAYTEST_HARNESS)", file=sys.stderr)
        return 2

    exe = ROOT / "target" / "debug" / "rewo.exe"
    if not a.no_build:
        exe = build()
    os.environ["PLAYTEST_REWO_EXE"] = str(exe)
    os.environ.setdefault("PLAYTEST_PORT", "25712")
    os.environ.setdefault("PLAYTEST_WAIT_G4", "3600")
    # tools/ holds a numbers.py that shadows the stdlib module numpy imports: drop it from the path.
    here = Path(__file__).resolve().parent
    sys.path[:] = [p for p in sys.path if Path(p or ".").resolve() != here]
    sys.path.insert(0, str(HARNESS))
    from harness import compare, paths, report, review  # noqa: E402  (the harness's own package)
    from harness.run import Blocked, run  # noqa: E402

    results, failed = [], []
    for p in chosen:
        print(f"[playtest] {p.stem}", flush=True)
        try:
            dirs = {}
            for kind, label in ((("rewo", "rewo"),) if a.rewo_only else (("puppet", "vanilla"), ("rewo", "rewo"))):
                d = run(p, label=label, client_kind=kind)
                status = json.loads((d / "run.json").read_text(encoding="utf-8"))["puppet_status"]
                print(f"  {label}: {status}  {d}", flush=True)
                if status != "ok":
                    failed.append(f"{p.stem}/{label}: {status}")
                dirs[label] = d
        except Blocked as e:
            print(f"blocked: {e}", file=sys.stderr)
            return 3
        if a.rewo_only or len(dirs) < 2 or any(f.startswith(p.stem + "/") for f in failed):
            continue
        out = paths.RUNS / f"compare-{datetime.now():%Y%m%d-%H%M%S}-{p.stem}-rewo"
        cmp = compare.compare(dirs["vanilla"], dirs["rewo"], out, labels=("vanilla", "Rewo"))
        rev = review.review(out) if a.review else None
        html = report.build(out)
        row = summarise(p.stem, cmp, rev)
        row["report"] = str(html)
        results.append(row)
        print(json.dumps(row, indent=2), flush=True)

    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    summary = {"when": stamp, "rewo": str(exe), "results": results, "failed": failed}
    (paths.RUNS / f"playtest-{stamp}.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(f"[playtest] {len(results)} compared, {len(failed)} failed runs; summary "
          f"{paths.RUNS / f'playtest-{stamp}.json'}")
    for f in failed:
        print(f"  FAILED {f}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
