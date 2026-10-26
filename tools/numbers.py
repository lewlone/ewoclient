#!/usr/bin/env python3
"""Rewrite the machine-derived numbers block in REWO_PLAN.md section 0.0.

The block sits between `<!-- numbers:begin -->` and `<!-- numbers:end -->`.
Everything inside it is generated; edit this script, never the block. It holds:

  * test totals per crate and overall, read from `cargo test` output;
  * the gate count (`EXPECTED_GATES` in tools/gates.py) and, when a gates.py
    `--json` result is given, how many rows of that sweep passed;
  * the `live --render-check` witness count (`EXPECTED_RENDER_CHECK_WITNESSES`
    in crates/rewo-app/src/live_cmd/render_check.rs);
  * the demo PNG hash (tools/demo_hash.txt);
  * per-gate witness counts: parsed from the gate's row in the gates.py JSON
    when the row carries one, otherwise read from the gate's pinned constant
    in crates/rewo-app/src/<gate>_cmd.rs (the "source" column says which).

Usage:
    python tools/numbers.py                    # run the tests and the gate sweep, then rewrite
    python tools/numbers.py --no-gates         # run only the tests (gate counts from source)
    python tools/numbers.py --from-logs test.log gates.json   # no re-run
    python tools/numbers.py --from-logs test.log              # no gate sweep result
    python tools/numbers.py --from-logs test.log gates.json --print   # show, don't write

`test.log` is the combined stdout+stderr of
`cargo test --workspace --no-fail-fast --features rewo-app/gates`; `gates.json`
is what `python tools/gates.py --json gates.json` writes.

Exit status: 0 when the block was written (or printed), 1 when the test log
shows failures or targets that did not build (the block is still written, with
the failures in it), 2 on bad input (missing markers, unreadable log, a log with
no `test result:` line at all).
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLAN = ROOT / "REWO_PLAN.md"
GATES_PY = ROOT / "tools" / "gates.py"
DEMO_HASH = ROOT / "tools" / "demo_hash.txt"
RENDER_CHECK = ROOT / "crates" / "rewo-app" / "src" / "live_cmd" / "render_check.rs"
APP_SRC = ROOT / "crates" / "rewo-app" / "src"
BEGIN = "<!-- numbers:begin -->"
END = "<!-- numbers:end -->"

TEST_CMD = ["cargo", "test", "--workspace", "--no-fail-fast", "--features", "rewo-app/gates"]

RUNNING = re.compile(r"^\s*Running (unittests )?(\S+) \((\S+)\)\s*$")
DOCTESTS = re.compile(r"^\s*Doc-tests (\S+)\s*$")
RESULT = re.compile(
    r"^test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured")
FAILED_TARGET = re.compile(r"^\s+`(-p \S+ .*)`\s*$")
COULD_NOT_COMPILE = re.compile(r"^error: could not compile `([^`]+)`")


# --------------------------------------------------------------------------
# Crate map: test-binary stem -> package name
# --------------------------------------------------------------------------

def crate_map() -> dict[str, str]:
    """Map every lib and bin target name in crates/* to its package name.

    A unittests binary is named after its target (`rewo_net-<hash>`, `rewo-<hash>`
    for rewo-app's `rewo` bin), and `Doc-tests` names the lib target.
    """
    out: dict[str, str] = {}
    for toml in sorted((ROOT / "crates").glob("*/Cargo.toml")):
        text = toml.read_text(encoding="utf-8")
        pkg = re.search(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', text, re.M | re.S)
        if not pkg:
            continue
        name = pkg.group(1)
        out[name.replace("-", "_")] = name
        for sect in re.finditer(r'^\[(?:lib|\[bin\])\][^\[]*?^name\s*=\s*"([^"]+)"', text,
                                re.M | re.S):
            out[sect.group(1).replace("-", "_")] = name
            out[sect.group(1)] = name
    return out


def binary_stem(path: str) -> str:
    """`target\\debug\\deps\\rewo_net-c9b8457c6a3cb049.exe` -> `rewo_net`."""
    stem = re.split(r"[\\/]", path)[-1]
    stem = re.sub(r"\.exe$", "", stem)
    return re.sub(r"-[0-9a-f]{8,}$", "", stem)


# --------------------------------------------------------------------------
# cargo test log
# --------------------------------------------------------------------------

def parse_tests(log: str) -> dict:
    """Sum `test result:` lines per crate.

    Integration-test binaries (`Running tests\\x.rs (...\\x-<hash>.exe)`) carry no
    crate name; cargo runs a package's targets together, lib/bin first, so they
    belong to the crate of the unittests binary before them.
    """
    cmap = crate_map()
    crates: dict[str, list[int]] = {}
    current = None
    pending = None  # crate the next `test result:` belongs to
    results = 0
    unknown: list[str] = []
    failed_targets: list[str] = []
    not_built: list[str] = []
    for line in log.splitlines():
        m = RUNNING.match(line)
        if m:
            is_unit, _src, binpath = m.groups()
            stem = binary_stem(binpath)
            if is_unit:
                current = cmap.get(stem) or cmap.get(stem.replace("-", "_"))
                if current is None:
                    unknown.append(stem)
                    current = stem
            pending = current or stem
            continue
        m = DOCTESTS.match(line)
        if m:
            pending = cmap.get(m.group(1), m.group(1))
            continue
        m = RESULT.match(line)
        if m:
            results += 1
            p, f, i, _ = (int(x) for x in m.groups())
            row = crates.setdefault(pending or "?", [0, 0, 0])
            row[0] += p
            row[1] += f
            row[2] += i
            continue
        m = FAILED_TARGET.match(line)
        if m:
            failed_targets.append(m.group(1))
            continue
        m = COULD_NOT_COMPILE.match(line)
        if m:
            not_built.append(m.group(1))
    if results == 0:
        raise ValueError("no `test result:` line in the test log")
    total = [sum(r[k] for r in crates.values()) for k in range(3)]
    return {"crates": crates, "total": total, "results": results,
            "unknown": unknown, "failed_targets": failed_targets, "not_built": not_built}


# --------------------------------------------------------------------------
# Source constants
# --------------------------------------------------------------------------

def expected_gates() -> list[str]:
    text = GATES_PY.read_text(encoding="utf-8")
    m = re.search(r"^EXPECTED_GATES = \[(.*?)^\]", text, re.M | re.S)
    if not m:
        raise ValueError("EXPECTED_GATES not found in tools/gates.py")
    return re.findall(r'"([a-z0-9]+)"', m.group(1))


def sub_gate_labels() -> list[str]:
    text = GATES_PY.read_text(encoding="utf-8")
    m = re.search(r"^SUB_GATES = \[(.*?)^\]", text, re.M | re.S)
    return re.findall(r'\(\s*"([a-z0-9:]+)"', m.group(1)) if m else []


def const(text: str, name: str) -> list[int]:
    return [int(v.replace("_", "")) for v in
            re.findall(rf"\bconst {name}: *usize *= *([0-9_]+)\s*;", text)]


def render_check_witnesses() -> int | None:
    vals = const(RENDER_CHECK.read_text(encoding="utf-8"), "EXPECTED_RENDER_CHECK_WITNESSES")
    return vals[0] if vals else None


def gate_source_count(gate: str) -> str:
    """The gate's pinned count as written in its source, or a short note."""
    path = APP_SRC / f"{gate}_cmd.rs"
    if not path.exists():
        return "(no source file)"
    text = path.read_text(encoding="utf-8")
    if gate == "soundshot":
        core = const(text, "EXPECTED_WITNESSES_CORE")
        audio = [v for v in const(text, "EXPECTED_WITNESSES_AUDIO") if v]
        if core:
            extra = f" / {core[0] + audio[0]} with `--features audio`" if audio else ""
            return f"{core[0]} (default build){extra}"
    for name, unit in (("EXPECTED_WITNESSES", ""), ("EXPECTED_MOB_VIEWS", " mob-views"),
                       ("EXPECTED_FRAMES", " frames")):
        vals = const(text, name)
        if vals:
            probes = const(text, "EXPECTED_PROBES")
            extra = f", {probes[0]} graded comparisons" if probes else ""
            return f"{vals[0]}{unit}{extra}"
    m = re.search(r"\bconst EXPECTED_FINGERPRINT: *\[u64; *(\d+)\]", text)
    if m:
        return f"fingerprint of {m.group(1)} counts"
    return "not pinned"


WITNESS_IN_DETAIL = [
    re.compile(r"PASS\s*(?:—|-)?\s*(\d+)\s+witnesses"),
    re.compile(r"PASS\s+(\d+)\s*/\s*(\d+)"),
    re.compile(r"(\d+)\s*/\s*(\d+)\s+witnesses"),
    re.compile(r"\b(\d+)\s+(?:witnesses|mob-views)\b"),
]


def count_from_detail(detail: str) -> str | None:
    for rx in WITNESS_IN_DETAIL:
        m = rx.search(detail)
        if m:
            return m.group(1)
    return None


# --------------------------------------------------------------------------
# Block
# --------------------------------------------------------------------------

def git_head() -> str:
    try:
        sha = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT,
                             capture_output=True, text=True, check=True).stdout.strip()
        dirty = subprocess.run(["git", "status", "--porcelain", "--untracked-files=no"],
                               cwd=ROOT, capture_output=True, text=True).stdout.strip()
        return sha + ("+dirty" if dirty else "")
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def build_block(tests: dict, gates_rows: list[dict] | None) -> list[str]:
    today = datetime.date.today().isoformat()
    lines = [
        BEGIN,
        "<!-- Generated by `python tools/numbers.py`; do not edit by hand, re-run it. -->",
        "",
        f"**Measured {today} at `{git_head()}`** by `tools/numbers.py` "
        "(tests: `cargo test --workspace --no-fail-fast --features rewo-app/gates`"
        + ("; gates: `python tools/gates.py --json`" if gates_rows is not None else "")
        + ").",
        "",
        "| Crate | passed | failed | ignored |",
        "|---|---:|---:|---:|",
    ]
    for name in sorted(tests["crates"]):
        p, f, i = tests["crates"][name]
        lines.append(f"| `{name}` | {p} | {f} | {i} |")
    p, f, i = tests["total"]
    lines.append(f"| **total** | **{p}** | **{f}** | **{i}** |")
    lines.append("")
    if tests["failed_targets"] or tests["not_built"]:
        bad = tests["not_built"] and ("did not build: " + ", ".join(
            f"`{c}`" for c in tests["not_built"]))
        fl = tests["failed_targets"] and ("failed targets: " + ", ".join(
            f"`{t}`" for t in tests["failed_targets"]))
        lines.append("**Test run not clean:** " + "; ".join(x for x in (bad, fl) if x) + ".")
        lines.append("")

    gates = expected_gates()
    subs = sub_gate_labels()
    rows_by_gate = {r.get("gate"): r for r in (gates_rows or [])}
    gate_line = (f"- **Gates:** {len(gates)} serverless gate commands pinned in "
                 f"`tools/gates.py` `EXPECTED_GATES`, plus {len(subs)} `mobshot` sub-gates "
                 "and the demo-hash row.")
    if gates_rows is not None:
        passed = sum(1 for r in gates_rows if r.get("status") == "PASS")
        gate_line += f" Last sweep: **{passed}/{len(gates_rows)}** rows passed."
        failing = [r["gate"] for r in gates_rows if r.get("status") != "PASS"]
        if failing:
            gate_line += " Not passing: " + ", ".join(f"`{g}`" for g in failing) + "."
    lines.append(gate_line)
    rc = render_check_witnesses()
    lines.append(f"- **`live --render-check`:** {rc if rc is not None else '?'} witnesses "
                 "(`EXPECTED_RENDER_CHECK_WITNESSES`, `crates/rewo-app/src/live_cmd/render_check.rs`).")
    demo = DEMO_HASH.read_text(encoding="utf-8").strip() if DEMO_HASH.exists() else "?"
    lines.append(f"- **Demo PNG hash:** `{demo}` (`tools/demo_hash.txt`).")
    lines.append("- **Packet coverage:** the table in `REWO_PACKET_COVERAGE.md` "
                 "(machine-checked by `ids.rs`).")
    lines.append("")
    head = "| Gate | witnesses | source |" if gates_rows is None else \
        "| Gate | witnesses | source | last sweep |"
    lines.append(head)
    lines.append("|---|---:|---|---|" if gates_rows is not None else "|---|---:|---|")
    for g in gates + subs:
        row = rows_by_gate.get(g)
        from_run = count_from_detail(row.get("detail", "")) if row else None
        if from_run is not None:
            count, src = from_run, "sweep output"
        elif ":" in g:
            count, src = "-", "mode of `mobshot`"
        else:
            count, src = gate_source_count(g), "source constant"
        cells = [f"`{g}`", count, src]
        if gates_rows is not None:
            cells.append(row.get("status", "?") if row else "not run")
        lines.append("| " + " | ".join(cells) + " |")
    if gates_rows is not None and "demo-hash" in rows_by_gate:
        lines.append(f"| `demo-hash` | - | `tools/demo_hash.txt` | "
                     f"{rows_by_gate['demo-hash'].get('status', '?')} |")
    lines.append(END)
    return lines


def rewrite_plan(block: list[str]) -> None:
    raw = PLAN.read_bytes().decode("utf-8")
    b = raw.find(BEGIN)
    e = raw.find(END)
    if b < 0 or e < 0 or e < b:
        raise ValueError(f"markers {BEGIN} / {END} not found (in order) in {PLAN.name}")
    # Keep the file's own line ending at the block.
    line_end = raw.find("\n", b)
    nl = "\r\n" if line_end > 0 and raw[line_end - 1] == "\r" else "\n"
    new = raw[:b] + nl.join(block) + raw[e + len(END):]
    PLAN.write_bytes(new.encode("utf-8"))


# --------------------------------------------------------------------------

def run_tests() -> str:
    print("$", " ".join(TEST_CMD), flush=True)
    # One merged stream: cargo writes its `Running ...` headers to stderr and
    # the test binaries write `test result:` to stdout, and `parse_tests`
    # attributes each result to the header before it. Capturing the two apart
    # and appending stderr put every header after every result (every crate
    # read as `?`).
    env = dict(os.environ, CARGO_TERM_COLOR="never")
    p = subprocess.run(TEST_CMD, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                       text=True, encoding="utf-8", errors="replace", env=env)
    return p.stdout


def run_gates() -> list[dict]:
    with tempfile.TemporaryDirectory() as d:
        out = Path(d) / "gates.json"
        cmd = [sys.executable, str(GATES_PY), "--json", str(out)]
        print("$", " ".join(cmd), flush=True)
        subprocess.run(cmd, cwd=ROOT)  # its exit status is recorded per row
        if not out.exists():
            raise ValueError("gates.py wrote no JSON (it exited before the sweep)")
        return json.loads(out.read_text(encoding="utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter,
                                 epilog="\n".join(__doc__.splitlines()[1:]))
    ap.add_argument("--from-logs", nargs="+", metavar=("TEST_LOG", "GATES_JSON"),
                    help="read a saved cargo test log (and optionally a gates.py --json "
                         "file) instead of running them")
    ap.add_argument("--no-gates", action="store_true",
                    help="when running, skip the gate sweep (needs a GPU and the local assets)")
    ap.add_argument("--print", dest="print_only", action="store_true",
                    help="print the block instead of rewriting REWO_PLAN.md")
    args = ap.parse_args()

    try:
        if args.from_logs:
            if len(args.from_logs) > 2:
                ap.error("--from-logs takes TEST_LOG and an optional GATES_JSON")
            log = Path(args.from_logs[0]).read_bytes().decode("utf-8", errors="replace")
            gates_rows = None
            if len(args.from_logs) == 2:
                gates_rows = json.loads(Path(args.from_logs[1]).read_text(encoding="utf-8"))
        else:
            log = run_tests()
            gates_rows = None if args.no_gates else run_gates()
        if gates_rows is not None and not isinstance(gates_rows, list):
            raise ValueError("gates JSON is not a list of rows")
        tests = parse_tests(log)
        block = build_block(tests, gates_rows)
        if args.print_only:
            if hasattr(sys.stdout, "reconfigure"):
                sys.stdout.reconfigure(encoding="utf-8")
            print("\n".join(block))
        else:
            rewrite_plan(block)
            print(f"rewrote the numbers block in {PLAN.name}: "
                  f"{tests['total'][0]} passed, {tests['total'][1]} failed")
    except (OSError, ValueError, json.JSONDecodeError) as e:
        print(f"numbers.py: {e}", file=sys.stderr)
        return 2
    if tests["unknown"]:
        print("warning: test binaries not mapped to a crate: " + ", ".join(tests["unknown"]),
              file=sys.stderr)
    bad = tests["total"][1] or tests["failed_targets"] or tests["not_built"]
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
