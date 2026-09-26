"""Shared harness for hand-written mutation batteries.

A battery is a list of `Mutation`s (exact-text find/replace in one source file)
plus the check commands that should turn red when a mutation is real. For each
mutation the harness: applies it, rebuilds, runs every check, restores the file
and verifies the restore **by bytes**. It always rebuilds the clean tree at the
end, so the binary on disk matches the source afterwards.

Rules it enforces (each one was learned from a battery that lied):
  * every battery starts with a no-op control that MUST survive — if the checks
    are already red, every mutant would otherwise read as "killed";
  * an anchor that matches 0 or 2+ times is SKIP, never a pass;
  * verdicts come from exit codes, never from grepping output text;
  * a timed-out check is TIMEOUT (stray processes are reaped), not a kill.

Usage (a battery is a small data file):

    from mutate_lib import Mutation, Check, run_battery
    FILE = "crates/rewo-world/src/book_view_screen.rs"
    MUTATIONS = [
        Mutation("click rect includes right edge", FILE,
                 "mx < s.x + s.w", "mx <= s.x + s.w", "half-open rect"),
    ]
    CHECKS = [
        Check(["cargo", "test", "-q", "-p", "rewo-world", "--lib", "--", "book_view"]),
        Check(["target/debug/rewo", "bookshot", "--check"]),
    ]
    if __name__ == "__main__":
        raise SystemExit(run_battery("m180", MUTATIONS, CHECKS))

For systematic mutation of pure crates, `cargo mutants -p <crate>` is usually
the better tool; this harness is for targeted claims that span crates/gates.
"""
from __future__ import annotations

import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


@dataclass
class Mutation:
    name: str
    path: str  # repo-relative
    find: str
    replace: str
    why: str = ""


@dataclass
class Check:
    cmd: list[str]
    timeout: int = 900


@dataclass
class Battery:
    build: list[str] = field(default_factory=lambda: ["cargo", "build", "-p", "rewo-app"])
    strays: tuple[str, ...] = ("rewo.exe",)


def _reap(strays) -> None:
    if os.name == "nt":
        for image in strays:
            subprocess.run(["taskkill", "/F", "/IM", image], capture_output=True)


def _run(cmd, timeout, strays):
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        _reap(strays)
        return None
    return p.returncode


def _checks_pass(checks, cfg) -> str:
    """'green', 'red', 'build-failed' or 'timeout'."""
    _reap(cfg.strays)
    code = _run(cfg.build, 1800, cfg.strays)
    if code is None:
        return "timeout"
    if code != 0:
        return "build-failed"
    for check in checks:
        code = _run(check.cmd, check.timeout, cfg.strays)
        if code is None:
            return "timeout"
        if code != 0:
            return "red"
    return "green"


def run_battery(tag: str, mutations: list[Mutation], checks: list[Check],
                cfg: Battery | None = None) -> int:
    cfg = cfg or Battery()
    first = mutations[0] if mutations else None
    control = Mutation("control: no change", first.path, first.find, first.find) if first else None
    plan = ([control] if control else []) + mutations
    results = []
    for m in plan:
        path = ROOT / m.path
        original = path.read_bytes()
        text = original.decode("utf-8")
        hits = text.count(m.find)
        if hits != 1:
            print(f"SKIP      {m.name}: anchor matched {hits} times")
            results.append((m, "SKIP"))
            continue
        try:
            path.write_bytes(text.replace(m.find, m.replace, 1).encode("utf-8"))
            state = _checks_pass(checks, cfg)
        finally:
            path.write_bytes(original)
            if path.read_bytes() != original:
                print(f"[{tag}] RESTORE FAILED for {m.path} - stopping")
                return 2
        is_control = m is control
        if state == "timeout":
            verdict = "TIMEOUT"
        elif is_control:
            verdict = "SURVIVED" if state == "green" else "CONTROL-RED"
        else:
            # A mutant that no longer compiles is caught by the type system,
            # which is a kill — but it grades the compiler, not the checks.
            verdict = {"green": "SURVIVED", "red": "KILLED", "build-failed": "BUILD-FAIL"}[state]
        results.append((m, verdict))
        print(f"{verdict:11} {m.name}")
        if verdict == "SURVIVED" and not is_control and m.why:
            print(f"            claim: {m.why[:110]}")

    if _run(cfg.build, 1800, cfg.strays) != 0:
        print(f"[{tag}] FINAL REBUILD FAILED - the binary does not match the tree")
        return 2

    ctrl_ok = bool(results) and results[0][1] == "SURVIVED"
    killed = sum(1 for _, v in results if v in ("KILLED", "BUILD-FAIL"))
    survivors = [m.name for m, v in results[1:] if v == "SURVIVED"]
    problems = [m.name for m, v in results if v in ("SKIP", "TIMEOUT", "CONTROL-RED")]
    print(f"[{tag}] {killed} killed, control {'ok' if ctrl_ok else 'FAILED'}, "
          f"survivors: {survivors}, problems: {problems}")
    return 0 if ctrl_ok and not problems and not survivors else 1


if __name__ == "__main__":
    sys.exit("import this module from a battery script; see the docstring")
