"""M144's mutation battery — the streaming path.

    python tools/m144_mutate.py

Same rules as its predecessors, and **run it alone** — see `m141g_mutate.py`'s
note on the interrupted-battery hazard, and `m141_mutate.py`'s on reading the
`test result:` line rather than the exit code (which cannot tell a failing test
from a failing build).

**Do not `git add` while this is running.** Each mutation restores the WORKING
TREE from a byte snapshot when it finishes, so `git status` comes back clean —
but `git add` writes a SEPARATE snapshot into the index at the moment it runs.
M142 committed a live mutant exactly that way.

## Everything here is graded by one crate, and that is the point

M143's battery had to route per file because its seam spanned four crates. This
one does not: the whole streaming path lives in `rewo-audio`, and its witnesses
run without a device *and* without an ogg for everything except the decoder
itself. That is the shape to aim for — the further a subsystem's grading can be
pulled into one crate's `--lib`, the cheaper it is to keep honest.

**Several witnesses here need the 26.2 asset store and SKIP without it**, which
`decode.rs`'s own module doc names as a real weakness. On a bare machine the
`OggStream` mutations below would report SURVIVED and mean nothing. Check for
`SKIPPED` in `cargo test -p rewo-audio --lib real_assets -- --nocapture` before
trusting this battery's decoder half.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
D = os.path.join("crates", "rewo-audio", "src", "decode.rs")
B = os.path.join("crates", "rewo-audio", "src", "buffers.rs")
M = os.path.join("crates", "rewo-audio", "src", "mixer.rs")
L = os.path.join("crates", "rewo-audio", "src", "live_sink.rs")

MUTATIONS = [
    (
        L,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// The engine's backend.",
        "/// The engine's backend (sic).",
        "SURVIVED",
    ),
    # --- the incremental stream (M144a) ------------------------------------
    (
        D,
        "a looping stream never restarts",
        "        if out.is_empty() && self.looping {",
        "        if false && out.is_empty() && self.looping {",
        "KILLED",
    ),
    (
        D,
        "the zero-sized read guard is gone (a read(0) rewinds a loop)",
        "        if samples == 0 {\n            return Ok(Vec::new());\n        }",
        "",
        "KILLED",
    ),
    (
        D,
        "an empty packet is read as end-of-stream",
        "                    return Ok(Some(b.samples().iter().copied().map(quantise).collect()));",
        "                    let v: Vec<i16> = b.samples().iter().copied().map(quantise).collect();\n                    if v.is_empty() {\n                        return Ok(None);\n                    }\n                    return Ok(Some(v));",
        "KILLED",
    ),
    (
        D,
        "the pending surplus is dropped between reads",
        "        let take = samples.min(self.pending.len());\n        Ok(self.pending.drain(..take).collect())",
        "        let take = samples.min(self.pending.len());\n        let out = self.pending.drain(..take).collect();\n        self.pending.clear();\n        Ok(out)",
        "KILLED",
    ),
    (
        D,
        "EQUIVALENT: restart does not clear pending (it is always empty there)",
        "        self.reader = reader;\n        self.pending.clear();",
        "        self.reader = reader;",
        "SURVIVED",
    ),
    (
        D,
        "the format is read off the first decoded packet's channel count",
        "            .map(|c| c.count() as u16)",
        "            .map(|_| 1u16)",
        "KILLED",
    ),
    # --- the buffer size and the seam (M144a) ------------------------------
    (
        B,
        "the sample size is 8 bits rather than 16",
        "    let bits = 16i32;",
        "    let bits = 8i32;",
        "KILLED",
    ),
    (
        B,
        "a boxed source inherits the refusing default instead of forwarding",
        # **The doc comment goes with it.** Deleting the fn alone leaves an
        # orphaned `///` and the file stops parsing, so the verdict is
        # BUILD-FAIL — a harness artefact rather than an answer about the code.
        # A mutation that cannot compile grades nothing.
        "    /// Forwarded explicitly. Inheriting the trait's default here would make a\n"
        "    /// boxed source silently refuse every stream while the source inside it\n"
        "    /// supported them — the failure would present as \"music never plays\" with a\n"
        "    /// perfectly good error message naming the wrong cause.\n"
        "    fn open_stream(&mut self, key: &str, looping: bool) -> Result<Box<dyn PcmStream>, String> {\n"
        "        (**self).open_stream(key, looping)\n    }",
        "",
        "KILLED",
    ),
    (
        B,
        "the library caches streams like static buffers",
        "        self.source.open_stream(key, looping)",
        "        let _ = looping;\n        Err(format!(\"{key}: cached\"))",
        "KILLED",
    ),
    # --- the mixer's queue (M144b) -----------------------------------------
    (
        M,
        "the queue is drained newest-first",
        "                    if let Some(next) = v.queue.pop_front() {",
        "                    if let Some(next) = v.queue.pop_back() {",
        "KILLED",
    ),
    (
        M,
        "the cursor resets at a join instead of carrying its fraction",
        "                        v.cursor -= src_frames as f64;",
        "                        v.cursor = 0.0;",
        "KILLED",
    ),
    (
        M,
        "an underrun kills a streaming voice",
        "                    } else if v.streaming {",
        "                    } else if false && v.streaming {",
        "KILLED",
    ),
    (
        M,
        "the first queued chunk is not promoted",
        # `if v.pcm.samples.is_empty()` also guards the render's underrun check,
        # so the anchor carries the line above it. An anchor that matches twice
        # is a mutation that NEVER RAN, which is not the same as one that
        # survived — the harness prints the count for exactly this.
        "                v.streaming = true;\n                if v.pcm.samples.is_empty() {",
        "                v.streaming = true;\n                if false {",
        "KILLED",
    ),
    (
        M,
        "the swapped buffer keeps the previous one's length",
        "                        src_frames = v.pcm.samples.len() / channels;",
        "                        src_frames = src_frames;",
        "KILLED",
    ),
    # --- the producer's pump (M144c) ---------------------------------------
    (
        L,
        "processed rounds up, so the queue refills a tick early",
        "        let processed = (consumed / buffer_frames).floor().max(0.0) as u64;",
        "        let processed = (consumed / buffer_frames).ceil().max(0.0) as u64;",
        "KILLED",
    ),
    (
        L,
        "the queue is one buffer deeper than QUEUED_BUFFER_COUNT",
        "            if stream.pushed_buffers - processed >= QUEUED_BUFFER_COUNT as u64 {",
        "            if stream.pushed_buffers - processed > QUEUED_BUFFER_COUNT as u64 {",
        "KILLED",
    ),
    (
        L,
        "the tick never runs updateStream",
        "        self.tick += 1;\n        self.update_streams();",
        "        self.tick += 1;",
        "KILLED",
    ),
    (
        L,
        "the attach does not prime the queue",
        "        self.pump(channel);\n    }\n\n    /// `removeProcessedBuffers`",
        "    }\n\n    /// `removeProcessedBuffers`",
        "KILLED",
    ),
    (
        L,
        "EQUIVALENT: the !ended guard cannot fire under the pump's own invariant",
        # `pump` refills whenever fewer than QUEUED_BUFFER_COUNT buffers remain
        # unplayed, so while a stream is alive `pushed` is always at least three
        # buffers ahead of `consumed` and the drain test below answers `false`
        # anyway. Kept because it makes `stopped()` correct INDEPENDENTLY of
        # whether the pump ran, which is what a future change to the pump's
        # cadence would rely on — the same reasoning `decode.rs`'s unreachable
        # `channels == 0` guard records.
        "            if !stream.ended {\n                return Some(false);\n            }",
        "",
        "SURVIVED",
    ),
    (
        L,
        "a stream is never reported stopped",
        "            return Some(state.consumed_frames(self.tick) >= pushed_frames);",
        "            let _ = pushed_frames;\n            return Some(false);",
        "KILLED",
    ),
    (
        L,
        "the buffer size stays in bytes rather than samples",
        "                    buffer_samples: bytes / 2,",
        "                    buffer_samples: bytes,",
        "KILLED",
    ),
    (
        L,
        "pitch does not affect how fast a stream is consumed",
        "        seconds * self.rate as f64 * self.pitch.max(0.01) as f64",
        "        seconds * self.rate as f64",
        "KILLED",
    ),
    (
        L,
        "EQUIVALENT: the processed cap cannot fire; it guards a u64 underflow",
        # Same invariant as above: `consumed` never runs past what was queued
        # while the stream is alive, so `processed` never exceeds
        # `pushed_buffers`. It stays because the very next line subtracts them as
        # `u64` — if the invariant ever broke, the difference would wrap to an
        # enormous number, the refill test would pass, and the stream would
        # starve in silence rather than fail loudly.
        "        let processed = processed.min(stream.pushed_buffers);",
        "",
        "SURVIVED",
    ),
    (
        L,
        "a stream failure is counted as a static one",
        "                self.streams_failed += 1;",
        "                self.unresolved += 1;",
        "KILLED",
    ),
]


def run_tests():
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note.

    One crate covers the whole milestone, so there is no routing table here.
    """
    for attempt in range(2):
        try:
            p = subprocess.run(
                ["cargo", "test", "-p", "rewo-audio", "--lib"],
                cwd=ROOT,
                capture_output=True,
                timeout=420,
            )
        except subprocess.TimeoutExpired:
            # A hung mutant takes the battery down and keeps the link output
            # open, so the NEXT build fails with linker error 1104 and looks
            # like a broken tree. Reap it and call the hang a kill.
            subprocess.run(["taskkill", "/F", "/IM", "rewo_audio-*.exe"], capture_output=True)
            return "failed"
        out = (p.stdout + p.stderr).decode("utf-8", "replace")
        if "test result: FAILED" in out:
            return "failed"
        if "test result: ok" in out and p.returncode == 0:
            return "ok"
        if attempt == 0:
            time.sleep(3)
            continue
        sys.stderr.write(out[-2000:] + "\n")
        return "build"
    return "build"


def main():
    paths = sorted({m[0] for m in MUTATIONS})
    snapshots = {p: io.open(os.path.join(ROOT, p), "rb").read() for p in paths}

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != "ok":
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for rel, name, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        text = snapshot.decode("utf-8")
        n = text.count(old)
        if n != 1:
            # An anchor matching twice is a mutation that NEVER RAN, which is
            # not the same as one that survived.
            print("%-64s ANCHOR MATCHED %d TIMES" % (name[:64], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            r = run_tests()
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-64s %-10s (want %-9s) %s"
            % (name[:64], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

    # Compared by BYTES, not by `git diff --quiet`: that cannot tell a leftover
    # mutation from ordinary uncommitted work, which M138's battery found.
    leftover = [
        p for p in paths if io.open(os.path.join(ROOT, p), "rb").read() != snapshots[p]
    ]
    print("-----")
    print(
        "files restored: %s"
        % ("no -- MUTATION LEFT ON DISK: %s" % leftover if leftover else "yes")
    )
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
