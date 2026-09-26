"""M143's mutation battery — wiring rewo-audio into `rewo live`.

    python tools/m143_mutate.py

Same rules as its predecessors, and **run it alone** — see `m141g_mutate.py`'s
note on the interrupted-battery hazard, and `m141_mutate.py`'s on reading the
`test result:` line rather than the exit code (which cannot tell a failing test
from a failing build).

**Do not `git add` while this is running.** Each mutation restores the WORKING
TREE from a byte snapshot when it finishes, so `git status` is clean afterwards
— but `git add` writes a SEPARATE snapshot into the index at the moment it
runs, and that moment can fall inside a mutation's window. M142 committed a
live mutant exactly that way. Stage before starting a battery or after its
summary, never across it.

**Routing matters here more than usual.** The seam spans four crates and no
single `cargo test` covers it: `sound_engine.rs`'s tee is graded by rewo-net's
own witnesses *and* by rewo-audio's end-to-end module, `live_sink.rs` only by
rewo-audio, `asset_index.rs` only by rewo-data, and the app wiring only by
`rewo-app --bins`. A battery that ran one crate would report most of these
SURVIVED and be wrong about it — M45's hazard, which `m142_mutate.py` met from
the other side when `assets::bake` turned out to be unreachable from
`cargo test` at all.

**The audio feature is deliberately NOT exercised.** `cargo test -p rewo-app
--features audio` would compile `audio_backend`'s enabled arm, but nothing can
run it without opening a device, and no test in this project does that. The
disabled arm's refusal is what gets graded.

## What the first run found (2026-08-12): two weak fixtures, no equivalents

Both survivors were holes in witnesses rather than mutants with no effect, and
both were invisible for the same reason — **the fixture drove the ordinary
eight-call sequence, and the ordinary sequence hides them.**

*The attach reset.* `Play` follows the attach and writes `played_at`
unconditionally, so a second `play_sequence` overwrites the reset a moment
after the mutation removed it. Attaching **without** a following `Play` is what
separates them, and it is a real state: a source with a fresh buffer and no
play is `AL_INITIAL`.

*The declined stream.* The witness counted the decline and checked nothing had
crossed the ring, and never asked `stopped()`. Without `dead` the channel is
held for the session — and the streaming pool is **five** channels, so five
records would wedge every streamed sound for the rest of the run. Declining a
feature must not cost the pool that serves it.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
S = os.path.join("crates", "rewo-audio", "src", "live_sink.rs")
B = os.path.join("crates", "rewo-audio", "src", "buffers.rs")
D = os.path.join("crates", "rewo-data", "src", "asset_index.rs")
P = os.path.join("crates", "rewo-app", "src", "live_cmd.rs")
A = os.path.join("crates", "rewo-app", "src", "audio_backend.rs")

MUTATIONS = [
    (
        S,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// The engine's backend.",
        "/// The engine's backend (sic).",
        "SURVIVED",
    ),
    # --- the tee (M143b) --------------------------------------------------
    # The headline. `SilentDevice::stopped` is unconditionally true, and
    # `schedule_tick` turns a true straight into `release`, which destroys the
    # source — so inheriting it makes every sound a 50 ms click.
    (
        E,
        "THE BLIP: the tee answers stopped() from the bookkeeping device",
        "        self.sink.stopped(channel).unwrap_or(true)",
        "        let _ = channel;\n        true",
        "KILLED",
    ),
    (
        E,
        "the no-opinion fallback flips, so an unknown channel is never released",
        "        self.sink.stopped(channel).unwrap_or(true)",
        "        self.sink.stopped(channel).unwrap_or(false)",
        "KILLED",
    ),
    (
        E,
        "release does not reach the backend (vanilla destroys the source)",
        "        self.book.release(channel);\n        self.sink.release(channel);",
        "        self.book.release(channel);",
        "KILLED",
    ),
    (
        E,
        "submit does not reach the backend",
        "        self.sink.submit(channel, &call);\n        self.book.submit(channel, call);",
        "        self.book.submit(channel, call);",
        "KILLED",
    ),
    (
        E,
        "the listener does not reach the backend",
        "        self.book.set_listener(transform);\n        self.sink.set_listener(transform);",
        "        self.book.set_listener(transform);",
        "KILLED",
    ),
    (
        E,
        "the listener does not reach the bookkeeping device (r45's counter)",
        "        self.book.set_listener(transform);\n        self.sink.set_listener(transform);",
        "        self.sink.set_listener(transform);",
        "KILLED",
    ),
    (
        E,
        "the backend's clock never advances",
        "        if let Some(s) = self.sink.as_mut() {\n            s.tick();\n        }",
        "",
        "KILLED",
    ),
    (
        E,
        "attach_sink stores nothing",
        "        self.sink = Some(sink);",
        "        let _ = sink;",
        "KILLED",
    ),
    # --- the backend (M143c) ----------------------------------------------
    (
        S,
        "stopped() is off by one at the boundary",
        "        Some(self.tick - played_at >= lifetime)",
        "        Some(self.tick - played_at > lifetime)",
        "KILLED",
    ),
    (
        S,
        "pitch is dropped, so it is not a playback rate",
        "        let seconds = frames as f64 / (state.rate as f64 * state.pitch.max(0.01) as f64);",
        "        let seconds = frames as f64 / state.rate as f64;",
        "KILLED",
    ),
    (
        S,
        "the buffer's own rate is replaced by a constant 44.1 kHz",
        "        let seconds = frames as f64 / (state.rate as f64 * state.pitch.max(0.01) as f64);",
        "        let seconds = frames as f64 / (44100.0 * state.pitch.max(0.01) as f64);",
        "KILLED",
    ),
    (
        S,
        "a looping source is allowed to stop",
        "        if state.looping {\n            return Some(false);\n        }",
        "",
        "KILLED",
    ),
    (
        S,
        "an unplayed source reports stopped (AL_INITIAL read as AL_STOPPED)",
        "        let (Some(played_at), Some(frames)) = (state.played_at, state.frames) else {\n            return Some(false);\n        };",
        "        let (Some(played_at), Some(frames)) = (state.played_at, state.frames) else {\n            return Some(true);\n        };",
        "KILLED",
    ),
    (
        S,
        "frames are counted as samples, so a stereo sound lasts twice as long",
        "                state.frames = Some(pcm.samples.len() as u64 / channels);",
        "                state.frames = Some(pcm.samples.len() as u64);",
        "KILLED",
    ),
    (
        S,
        "an attach does not rewind this side's clock",
        "                state.played_at = None;\n                self.push(Command::Attach(channel, pcm));",
        "                self.push(Command::Attach(channel, pcm));",
        "KILLED",
    ),
    (
        S,
        "the resolved samples never cross the ring",
        "                self.push(Command::Attach(channel, pcm));",
        "                let _ = pcm;",
        "KILLED",
    ),
    (
        S,
        "release does not stop the voice",
        "        self.push(Command::Channel(channel, ChannelCall::Stop));",
        "",
        "KILLED",
    ),
    (
        S,
        "the tick never advances",
        "        self.tick += 1;",
        "",
        "KILLED",
    ),
    (
        S,
        "a failed attach is not recorded as dead",
        "                state.frames = None;\n                state.dead = true;\n                self.unresolved += 1;",
        "                state.frames = None;\n                self.unresolved += 1;",
        "KILLED",
    ),
    (
        S,
        "a declined stream is not recorded as dead",
        "        state.frames = None;\n        state.dead = true;\n        log::debug!",
        "        state.frames = None;\n        log::debug!",
        "KILLED",
    ),
    # --- the buffer library (M143c) ---------------------------------------
    (
        B,
        "the cache never hits, so every play re-decodes",
        "        if !self.cache.contains_key(key) {",
        "        if true {",
        "KILLED",
    ),
    # --- the asset index (M143a) ------------------------------------------
    (
        D,
        "the key is treated as a path instead of resolving through the hash",
        "        let path = crate::sounds_json::object_path(assets_root, hash);",
        "        let path = assets_root.join(key);",
        "KILLED",
    ),
    (
        D,
        "a malformed index yields an empty map instead of an error",
        '            .ok_or_else(|| format!("{}: no objects", path.display()))?;',
        "            .cloned()\n            .unwrap_or_default();\n        let objects = &objects;",
        "KILLED",
    ),
    # --- the app wiring (M143e/f) -----------------------------------------
    (
        P,
        "an opened backend is not attached",
        "        Ok(sink) => live.attach_sink(sink),",
        "        Ok(_sink) => {}",
        "KILLED",
    ),
    (
        P,
        "a failed open attaches nothing and says nothing (the silent downgrade)",
        'Err(e) => log::error!("audio: --audio was requested and no device was opened: {e}"),',
        "Err(_e) => {}",
        "SURVIVED",
    ),
    (
        A,
        "the refusal stops naming the build command",
        '    Err("this build has no audio stack linked (cargo build -p rewo-app --features audio)".into())',
        '    Err("audio unavailable".into())',
        "KILLED",
    ),
    # --- composition roots, which no test can reach (must SURVIVE) ---------
    (
        P,
        "COMPOSITION ROOT: --audio never opens anything (must SURVIVE)",
        "        attach_backend(&mut live, crate::audio_backend::open(version));",
        "        let _ = version;",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the env fallback is dropped (must SURVIVE)",
        '    args.audio || std::env::var("REWO_AUDIO").map(|v| v == "1").unwrap_or(false)',
        "    args.audio",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the diagnostics are never reported (must SURVIVE)",
        "            if audio != self.last_audio {",
        "            if false && audio != self.last_audio {",
        "SURVIVED",
    ),
]

# Which crates grade which file. A mutation is only run against the suites that
# can see it — and the point of listing them per file is that no single suite
# sees them all.
SUITES = {
    E: [
        ["cargo", "test", "-p", "rewo-net", "--lib"],
        ["cargo", "test", "-p", "rewo-audio", "--lib"],
    ],
    S: [["cargo", "test", "-p", "rewo-audio", "--lib"]],
    B: [["cargo", "test", "-p", "rewo-audio", "--lib"]],
    D: [["cargo", "test", "-p", "rewo-data", "--lib"]],
    P: [["cargo", "test", "-p", "rewo-app", "--bins"]],
    A: [["cargo", "test", "-p", "rewo-app", "--bins"]],
}


def run_tests(rel=None):
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note.

    The exit code alone cannot distinguish a failing test from a failing build,
    and treating a build failure as a kill would make a mutation that does not
    compile look like a witness doing its job.
    """
    suites = SUITES[rel] if rel else [s for v in SUITES.values() for s in v]
    for attempt in range(2):
        outs, rcs = [], []
        for args in suites:
            try:
                p = subprocess.run(args, cwd=ROOT, capture_output=True, timeout=420)
            except subprocess.TimeoutExpired:
                # A hung mutant takes the battery down and keeps the link output
                # open, so the NEXT build fails with linker error 1104 and looks
                # like a broken tree. Reap it and call the hang a kill.
                for exe in ("rewo_net-*.exe", "rewo_audio-*.exe", "rewo_data-*.exe", "rewo.exe"):
                    subprocess.run(["taskkill", "/F", "/IM", exe], capture_output=True)
                return "failed"
            outs.append((p.stdout + p.stderr).decode("utf-8", "replace"))
            rcs.append(p.returncode)
        joined = "\n".join(outs)
        if "test result: FAILED" in joined:
            return "failed"
        if all("test result: ok" in o for o in outs) and all(r == 0 for r in rcs):
            return "ok"
        if attempt == 0:
            time.sleep(3)
            continue
        sys.stderr.write(joined[-2000:] + "\n")
        return "build"
    return "build"


def main():
    paths = sorted({m[0] for m in MUTATIONS})
    snapshots = {p: io.open(os.path.join(ROOT, p), "rb").read() for p in paths}

    print("BASELINE (unmutated, every suite) ...", end=" ", flush=True)
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
            # not the same as one that survived. Reported as its own outcome.
            print("%-64s ANCHOR MATCHED %d TIMES" % (name[:64], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            r = run_tests(rel)
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
