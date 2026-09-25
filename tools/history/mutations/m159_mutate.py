"""M159's mutation battery — the streaming decode on the sound-engine thread.

Run: python tools/m159_mutate.py [lo] [hi]

Routed through `cargo test -p rewo-audio --lib`, which is where M159's claims
live. **That is the choice M158's gotcha 0d is about**, so it is worth saying
why it is right here and was wrong there: M158's subject was a `*shot` gate and
its battery went through `cargo test`, which could not reach it. M159's subject
is a crate's own behaviour and every witness is a unit test, so this is the
check that covers the claims.

Discipline: a no-op control that must SURVIVE, exit codes rather than
substrings, a per-mutation timeout so a hang is a KILL rather than an outage
that leaves the mutant on disk, and a restore verified by BYTES.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SINK = os.path.join(ROOT, "crates/rewo-audio/src/live_sink.rs")
WORK = os.path.join(ROOT, "crates/rewo-audio/src/stream_worker.rs")

MUTATIONS = [
    ("control: no change", SINK, "    epoch: u64,", "    epoch: u64,",
     "MUST SURVIVE — otherwise every verdict below is vacuous"),

    # ---- the milestone's own claim -------------------------------------
    ("the decode goes back inline even with a worker",
     SINK,
     "        if self.streams.is_some() {\n            let state = self.channels.entry(channel).or_default();",
     "        if false {\n            let state = self.channels.entry(channel).or_default();",
     "the whole milestone: every read back on the client tick"),

    # ---- the epoch, and the hazard it exists for -----------------------
    ("the epoch never advances, so a stale chunk looks current",
     SINK,
     "            state.epoch = state.epoch.wrapping_add(1);",
     "            state.epoch = state.epoch;",
     "a chunk from a replaced stream would splice into the new one"),
    ("a landing is applied without checking the epoch",
     SINK,
     "            if state.epoch != key.epoch {",
     "            if false {",
     "same hazard, from the landing side"),

    # ---- the queue invariant under latency -----------------------------
    ("the queue gate counts what LANDED rather than what was asked for",
     SINK,
     "            let outstanding = stream.requested_buffers - processed;",
     "            let outstanding = stream.pushed_buffers - processed;",
     "one slow read becomes a fresh four-buffer request on every tick",
     ),
    ("a request is recorded even when the worker refused it",
     WORK,
     "        if self.tx.send(StreamRequest::Pump { key, buffers }).is_err() {",
     "        if false {",
     "a dead worker would leave the channel believing its queue was full"),
    ("the request is not recorded at all",
     SINK,
     "                if w.pump(skey, want as usize) {\n                    stream.requested_buffers += want;\n                }",
     "                w.pump(skey, want as usize);",
     "in-flight requests would be invisible and re-asked every tick"),

    # ---- the worker's own bookkeeping ----------------------------------
    ("an ended stream leaves its requests counted in flight",
     WORK,
     "                StreamEvent::Ended { key } | StreamEvent::OpenFailed { key, .. } => {\n                    self.inflight.remove(key);\n                }",
     "                StreamEvent::Ended { .. } | StreamEvent::OpenFailed { .. } => {}",
     "a re-used key would look permanently busy"),
    ("exhaustion is not reported, so a finite stream never ends",
     WORK,
     "                                        Ok(c) if c.is_empty() => {\n                                            finished = true;\n                                            break;\n                                        }",
     "                                        Ok(c) if c.is_empty() => {\n                                            break;\n                                        }",
     "a finished track would hold its channel forever"),

    # ---- the open --------------------------------------------------------
    ("a failed open leaves the channel alive rather than dead",
     SINK,
     "                E::OpenFailed { error, .. } => {\n                    state.stream = None;\n                    state.frames = None;\n                    state.dead = true;",
     "                E::OpenFailed { error, .. } => {\n                    state.stream = None;\n                    state.frames = None;\n                    state.dead = false;",
     "an unopenable stream would wedge its channel for the session"),
    ("the open does not prime the queue",
     SINK,
     # Anchored on the statement alone, which is unique. The first version
     # quoted the comment above it, and extending that comment turned the
     # mutation into a silent SKIP — the anchor-count guard is what reported it.
     "                    self.pump(key.channel);",
     "                    let _ = key.channel;",
     "EXPECTED SURVIVOR, proven equivalent: `tick` runs `poll_streams` and "
     "then `update_streams`, so a stream opened in a landing is pumped by the "
     "tick's own sweep before that same tick returns. Kept because "
     "`attachBufferStream` primes its own queue and a reader should find that "
     "at the attach."),
]


def run():
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", "rewo-audio", "--lib"],
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
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m159] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control")

    results = []
    for name, path, find, repl, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl)
            )
            survived, reason = run()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
        results.append((name, verdict, why))

    print()
    if not results or results[0][1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        return 2
    killed = sum(1 for _, v, _ in results[1:] if v == "KILLED")
    total = len(results) - 1
    print(f"control SURVIVED (battery is valid) - {killed}/{total} killed")
    for name, verdict, why in results[1:]:
        if verdict != "KILLED":
            print(f"  {verdict}: {name}\n    would mean: {why}")
    # One expected, proven-equivalent survivor (the redundant prime).
    return 0 if killed >= total - 1 else 1


if __name__ == "__main__":
    sys.exit(main())
