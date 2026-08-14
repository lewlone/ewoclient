"""M156's mutation battery — the static decode worker.

Run: python tools/m156_mutate.py

Same discipline as m152/m154/m155: a no-op control that must SURVIVE, exit
codes rather than substrings, a per-mutation timeout, and a restore verified by
BYTES.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DW = "crates/rewo-audio/src/decode_worker.rs"
BUF = "crates/rewo-audio/src/buffers.rs"
LS = "crates/rewo-audio/src/live_sink.rs"

MUTATIONS = [
    (
        "control: no change",
        DW,
        "pub fn inflight_count(&self) -> usize {",
        "pub fn inflight_count(&self) -> usize {",
        "rewo-audio",
        "",
        "MUST SURVIVE — otherwise every KILLED below is vacuous",
    ),
    (
        "worker: no in-flight dedup (a future-cache becomes a result-cache)",
        DW,
        "        if self.inflight.contains(key) {\n            return false;\n        }",
        "        if false {\n            return false;\n        }",
        "rewo-audio",
        "",
        "N plays of a first-time sound would decode it N times",
    ),
    (
        "worker: a dead worker still marks the key in flight",
        DW,
        "        if self.tx.send(key.to_string()).is_err() {\n            // The worker is gone. Do NOT mark it in flight — a key recorded as\n            // pending against a dead worker would never complete and would\n            // wedge its channel silently.\n            return false;\n        }",
        "        let _ = self.tx.send(key.to_string());",
        "rewo-audio",
        "",
        "a key would be pending forever and its channel would never sound",
    ),
    (
        "worker: a landing does not clear the flight mark",
        DW,
        "            self.inflight.remove(&d.0);",
        "            let _ = &d.0;",
        "rewo-audio",
        "",
        "a key could never be decoded a second time after a cache clear",
    ),
    (
        "library: a pending decode reports Ready(Err) rather than Pending",
        BUF,
        "            Some(w) => {\n                w.request(key);\n                BufferState::Pending\n            }",
        '            Some(w) => {\n                w.request(key);\n                BufferState::Ready(Err("pending".into()))\n            }',
        "rewo-audio",
        "",
        "every first play of a sound would be reported as a missing asset",
    ),
    (
        "hazard 1: a deferred attach wipes the play stamp",
        LS,
        "                state.played_at = if deferred && state.played_at.is_some() {\n                    Some(now)\n                } else {\n                    None\n                };",
        "                state.played_at = None;",
        "rewo-audio",
        "",
        "stopped() would answer false forever and the channel pool would run dry",
    ),
    (
        "hazard 2: a landing attaches to a released channel",
        LS,
        "                .filter(|(_, s)| s.pending.as_deref() == Some(key.as_str()))",
        "                .filter(|(_, s)| s.pending.is_some() || key.is_empty())",
        "rewo-audio",
        "",
        "a channel the engine gave away would be resurrected by a late decode",
    ),
    (
        "tick: decodes are never polled",
        LS,
        "        self.poll_decodes();\n        self.update_streams();",
        "        self.update_streams();",
        "rewo-audio",
        "",
        "a deferred attach would never complete and no sound would ever play",
    ),
]


def run(crate, filt):
    args = ["cargo", "test", "-p", crate, "--lib"]
    if filt:
        args.append(filt)
    try:
        p = subprocess.run(
            args,
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=420,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    if p.returncode != 0:
        why = (
            "build failed"
            if "error[E" in p.stderr or "could not compile" in p.stderr
            else "tests failed"
        )
        return False, why
    if "test result: ok" not in p.stdout:
        return False, "no test result line"
    return True, "passed"


def main():
    results = []
    for name, path, find, repl, crate, filt, why in MUTATIONS:
        full = os.path.join(ROOT, path)
        original = io.open(full, encoding="utf-8", newline="").read()
        if original.count(find) != 1:
            print(f"SKIP      {name}: anchor matched {original.count(find)} times")
            results.append((name, "SKIP", why))
            continue
        try:
            io.open(full, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl)
            )
            survived, reason = run(crate, filt)
        finally:
            io.open(full, "w", encoding="utf-8", newline="").write(original)
            assert io.open(full, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
        results.append((name, verdict, why))

    print()
    if results[0][1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        return 2
    killed = sum(1 for _, v, _ in results[1:] if v == "KILLED")
    total = len(results) - 1
    print(f"control SURVIVED (battery is valid) - {killed}/{total} killed")
    for name, verdict, why in results[1:]:
        if verdict != "KILLED":
            print(f"  {verdict}: {name}\n    would mean: {why}")
    return 0 if killed == total else 1


if __name__ == "__main__":
    sys.exit(main())
