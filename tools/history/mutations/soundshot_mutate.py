"""`soundshot`'s mutation battery — does each witness detect the break it claims?

    python tools/soundshot_mutate.py            # everything
    python tools/soundshot_mutate.py 0 12       # a slice, by index

**Run it alone**, and **do not `git add` while it is running**: each mutation
restores the working TREE from a byte snapshot, so `git status` comes back
clean, but `git add` writes a separate snapshot into the index at the moment it
runs. M142 committed a live mutant that way.

## What is different about this one

Every other battery in `tools/` mutates production code and grades it with
`cargo test`. This one mutates production code and grades it with **the gate**,
because the gate is the subject: `soundshot` exists to catch exactly these
breaks, and a witness that does not is a witness that would not have noticed.

So every mutation is **gate-routed**, which brings `m148_mutate.py`'s hazard to
every entry rather than to a few: the restore puts the source back **without
rebuilding**, so `target/debug/rewo.exe` is left as the last mutant's binary and
the next gate run grades that mutant against a clean tree. It presents as a gate
failing with `git status` clean, which is the most confusing possible shape.
This rebuilds before it exits.

## Why one build configuration

The gate has two locks — 27 witnesses by default and 47 with `--features audio`
— and the audio configuration is a strict superset, so every mutation is graded
by the audio build. A break that only the core layers see is still seen there.
(The consequence: this battery does **not** grade the default lock's own
arithmetic. That is a unit-level claim about a constant, not a behaviour.)

## Hazards handled, each of which has cost a run somewhere in this project

* **A hang takes the battery down and its `finally` never runs**, leaving the
  mutation on disk; then the hung test binary holds the link output and the next
  build fails with linker error 1104, looking like a broken tree. Every child
  gets a timeout, a hang counts as KILLED, and strays are reaped.
* **An anchor that matches 0 or 2 times is a stale battery, not a pass.** Every
  anchor is checked for exactly one match before anything is written.
* **A battery run against an already-failing command reads KILLED for every
  entry** (M109). The CONTROL is a comment-only edit that must SURVIVE, and the
  baseline is run first.
* **`git diff --quiet` cannot tell a leftover mutation from uncommitted work**
  (M138a). The restore check compares file BYTES against the snapshot.
* **Read exit codes, never substrings.** The gate prints `ok` on every passing
  witness line, so grepping for failure text is unreliable; `--check` returns
  non-zero on any failure or on a witness-count mismatch.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

SND = os.path.join("crates", "rewo-net", "src", "sounds.rs")
LIB = os.path.join("crates", "rewo-net", "src", "lib.rs")
ENG = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
INS = os.path.join("crates", "rewo-net", "src", "sound_instance.rs")
SEV = os.path.join("crates", "rewo-data", "src", "sound_events.rs")
SJS = os.path.join("crates", "rewo-data", "src", "sounds_json.rs")
LIVE = os.path.join("crates", "rewo-app", "src", "live_cmd.rs")
GATE = os.path.join("crates", "rewo-app", "src", "soundshot_cmd.rs")
QNT = os.path.join("crates", "rewo-audio", "src", "quantise.rs")
BUF = os.path.join("crates", "rewo-audio", "src", "buffers.rs")
DEC = os.path.join("crates", "rewo-audio", "src", "decode.rs")
MIX = os.path.join("crates", "rewo-audio", "src", "mixer.rs")

# (file, name, old, new, expected)
MUTATIONS = [
    (
        GATE,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "//! # The five layers",
        "//! # The five layers of this gate",
        "SURVIVED",
    ),
    # ---- (w) the wire -----------------------------------------------------
    (
        SND,
        "w: the Holder is read RAW instead of id + 1",
        "            Ok(SoundRef::Registry(raw - 1))",
        "            Ok(SoundRef::Registry(raw))",
        "KILLED",
    ),
    (
        SND,
        "w: the fixed-point position divides by 16 instead of 8",
        "    (v as f32 / LOCATION_ACCURACY) as f64",
        "    (v as f32 / (LOCATION_ACCURACY * 2.0)) as f64",
        "KILLED",
    ),
    (
        SND,
        "w: sound_entity reads its id as a fixed i32 like its sibling",
        "        let entity_id = r.varint()?;",
        "        let entity_id = r.i32()?;",
        "KILLED",
    ),
    (
        SND,
        "w: stop_sound reads the name before the source",
        "        let source = if flags & STOP_HAS_SOURCE != 0 {\n"
        "            Some(SoundSource::read(r)?)\n"
        "        } else {\n"
        "            None\n"
        "        };\n"
        "        let name = if flags & STOP_HAS_SOUND != 0 {\n"
        "            Some(r.identifier()?)\n"
        "        } else {\n"
        "            None\n"
        "        };",
        "        let name = if flags & STOP_HAS_SOUND != 0 {\n"
        "            Some(r.identifier()?)\n"
        "        } else {\n"
        "            None\n"
        "        };\n"
        "        let source = if flags & STOP_HAS_SOURCE != 0 {\n"
        "            Some(SoundSource::read(r)?)\n"
        "        } else {\n"
        "            None\n"
        "        };",
        "KILLED",
    ),
    (
        SEV,
        "w: THE ALPHABETISATION TRAP -- ids come from iteration position",
        "            if let Some(id) = entry.get(\"protocol_id\").and_then(|i| i.as_i64()) {",
        "            let _ = entry;\n            if let Some(id) = Some(by_id.len() as i64) {",
        "KILLED",
    ),
    (
        LIB,
        "w: a level_event sound lands at the block CORNER",
        "        x: x as f64 + 0.5,\n        y: y as f64 + 0.5,\n        z: z as f64 + 0.5,",
        "        x: x as f64,\n        y: y as f64,\n        z: z as f64,",
        "KILLED",
    ),
    (
        LIB,
        "w: the row's own volume is dropped for a flat 1.0",
        "        volume: row.volume.unwrap_or(1.0),",
        "        volume: 1.0,",
        "KILLED",
    ),
    # ---- (s) resolution ---------------------------------------------------
    (
        LIVE,
        "s: build_sounds degrades to an empty index instead of failing closed",
        "        Err(e) if strict => {\n            panic!(",
        "        Err(e) if strict && false => {\n            panic!(",
        "KILLED",
    ),
    (
        SJS,
        "s: the weight walk uses <= 0 instead of < 0",
        "            if index < 0 {",
        "            if index <= 0 {",
        "KILLED",
    ),
    (
        SJS,
        "s: a redirect resolves without drawing again",
        "                let inner = self.pick(target, rng, depth + 1)?;",
        "                let inner = self.resolve(target.sounds.first()?, rng, depth + 1)?;",
        "KILLED",
    ),
    (
        SJS,
        "s: a redirect's weight comes from the target, not the outer",
        "                    // The redirect's own weight, not the target's — see the\n"
        "                    // `ResolvedSound` docs.\n"
        "                    weight: sound.weight,",
        "                    weight: inner.weight,",
        "KILLED",
    ),
    (
        SJS,
        "s: a redirect's attenuation comes from the outer, not the inner",
        "                    attenuation_distance: inner.attenuation_distance,",
        "                    attenuation_distance: sound.attenuation_distance,",
        "KILLED",
    ),
    (
        SJS,
        "s: a redirect's volume is the outer's rather than the product",
        "                    volume: inner.volume * sound.volume,",
        "                    volume: sound.volume,",
        "KILLED",
    ),
    (
        SJS,
        "s: validateSoundResource keeps a variant whose file is absent",
        "                if !files.has(&format!(\"{ns}/sounds/{path}.ogg\")) {",
        "                if false && !files.has(&format!(\"{ns}/sounds/{path}.ogg\")) {",
        "KILLED",
    ),
    (
        SJS,
        "s: an unregistered redirect target weighs its own declared weight",
        "            SoundType::Event => self\n"
        "                .events\n"
        "                .get(&sound.name)\n"
        "                .map_or(0, |t| self.total_weight_at(t, depth + 1)),",
        "            SoundType::Event => self\n"
        "                .events\n"
        "                .get(&sound.name)\n"
        "                .map_or(sound.weight, |t| self.total_weight_at(t, depth + 1)),",
        "KILLED",
    ),
    (
        SJS,
        "s: intentionally_empty is looked up in the registry like anything else",
        "        if event == INTENTIONALLY_EMPTY_SOUND {",
        "        if false && event == INTENTIONALLY_EMPTY_SOUND {",
        "KILLED",
    ),
    # ---- (a) arithmetic and sequence --------------------------------------
    (
        ENG,
        "a: setVolume is submitted before setPitch",
        "        device.submit(channel, ChannelCall::SetPitch(pitch));\n"
        "        device.submit(channel, ChannelCall::SetVolume(gain));",
        "        device.submit(channel, ChannelCall::SetVolume(gain));\n"
        "        device.submit(channel, ChannelCall::SetPitch(pitch));",
        "KILLED",
    ),
    (
        ENG,
        "a: play() is submitted before the attach",
        "        device.submit(channel, ChannelCall::Play);",
        "",
        "KILLED",
    ),
    (
        INS,
        "a: master is multiplied into itself",
        "        if source == SoundSource::Master {\n            self.slider(SoundSource::Master)\n        } else {",
        "        if false {\n            self.slider(SoundSource::Master)\n        } else {",
        "KILLED",
    ),
    (
        INS,
        "a: the range uses the CLAMPED volume, collapsing a jukebox to 16 blocks",
        "    instance_volume.max(1.0) * sound_attenuation_distance as f32",
        "    instance_volume.min(1.0).max(1.0) * sound_attenuation_distance as f32",
        "KILLED",
    ),
    (
        ENG,
        "a: the MUSIC escape from the zero-volume drop is removed",
        "            if !instance.can_start_silent && instance.source != SoundSource::Music {",
        "            if !instance.can_start_silent {",
        "KILLED",
    ),
    (
        ENG,
        "a: StartedSilently collapses into Started",
        "            if started_silently {\n                PlayResult::StartedSilently\n            } else {",
        "            if false {\n                PlayResult::StartedSilently\n            } else {",
        "KILLED",
    ),
    (
        ENG,
        "a: MIN_SOURCE_LIFETIME is zero, so a stopped channel is reclaimed at once",
        "pub const MIN_SOURCE_LIFETIME: i32 = 20;",
        "pub const MIN_SOURCE_LIFETIME: i32 = 0;",
        "KILLED",
    ),
    (
        ENG,
        "a: the channel budget never refuses",
        "        if *used >= limit {\n            return false;\n        }",
        "        if *used >= limit && false {\n            return false;\n        }",
        "KILLED",
    ),
    (
        ENG,
        "a: the pool split ROUNDS where Mth.sqrt's cast truncates",
        "    let streaming = ((total_channel_count as f32).sqrt() as i32).clamp(2, 8);",
        "    let streaming = ((total_channel_count as f32).sqrt().round() as i32).clamp(2, 8);",
        "KILLED",
    ),
    (
        ENG,
        "a: a streaming loop also loops on the source",
        "        device.submit(channel, ChannelCall::SetLooping(looping && !streaming));",
        "        device.submit(channel, ChannelCall::SetLooping(looping));",
        "KILLED",
    ),
    (
        ENG,
        "a: entity_silent is ignored, so a /data-silenced mob is audible",
        "        if !instance.can_play_sound(silent) {",
        "        if !instance.can_play_sound(false) {",
        "KILLED",
    ),
    # ---- (d) decode -------------------------------------------------------
    (
        QNT,
        "d: the quantisation drops its -0.5 bias",
        "    let scaled = sample * 32767.5 - 0.5;",
        "    let scaled = sample * 32767.5;",
        "KILLED",
    ),
    (
        QNT,
        "d: the multiplier is 32767 rather than 32767.5",
        "    let scaled = sample * 32767.5 - 0.5;",
        "    let scaled = sample * 32767.0 - 0.5;",
        "KILLED",
    ),
    (
        QNT,
        "d: the cast FLOORS instead of truncating toward zero",
        "    (scaled as i32).clamp(-32768, 32767) as i16",
        "    (scaled.floor() as i32).clamp(-32768, 32767) as i16",
        "KILLED",
    ),
    (
        BUF,
        "d: a failed decode is retried rather than cached",
        "        if !self.cache.contains_key(key) {\n            let decoded = self.source.open(key).map(Arc::new);\n            self.cache.insert(key.to_string(), decoded);\n        }",
        "        if !self.cache.contains_key(key) || self.cache[key].is_err() {\n            let decoded = self.source.open(key).map(Arc::new);\n            self.cache.insert(key.to_string(), decoded);\n        }",
        "KILLED",
    ),
    (
        BUF,
        "d: a static buffer is not cached at all",
        "        if !self.cache.contains_key(key) {",
        "        if true {",
        "KILLED",
    ),
    (
        DEC,
        "d: a looping stream restarts on a SHORT read rather than an empty one",
        "        if out.is_empty() && self.looping {",
        "        if out.len() < samples && self.looping {",
        "KILLED",
    ),
    # ---- (m) the mixer ----------------------------------------------------
    (
        MIX,
        "m: render accumulates into its output buffer instead of clearing it",
        "        for s in out.iter_mut() {\n            *s = 0.0;\n        }\n        let frames = out.len() / 2;",
        "        let frames = out.len() / 2;",
        "KILLED",
    ),
    (
        MIX,
        "m: the pan law is a no-op, so everything is centred",
        "    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;\n    (angle.cos(), angle.sin())",
        "    let _ = pan;\n    (1.0, 1.0)",
        "KILLED",
    ),
    (
        ENG,
        "m: right() is up x forward, mirroring the stereo image",
        "        [\n            f[1] * u[2] - f[2] * u[1],\n            f[2] * u[0] - f[0] * u[2],\n            f[0] * u[1] - f[1] * u[0],\n        ]",
        "        [\n            u[1] * f[2] - u[2] * f[1],\n            u[2] * f[0] - u[0] * f[2],\n            u[0] * f[1] - u[1] * f[0],\n        ]",
        "KILLED",
    ),
    (
        MIX,
        "m: AL_SOURCE_RELATIVE is ignored, so a UI sound fades as you walk",
        "            let rel = if v.relative {",
        "            let rel = if false {",
        "KILLED",
    ),
    (
        MIX,
        "m: pitch is dropped from the rate conversion",
        "            let step = (v.pcm.sample_rate as f64 / self.out_rate as f64) * v.pitch as f64;",
        "            let step = v.pcm.sample_rate as f64 / self.out_rate as f64;",
        "KILLED",
    ),
    (
        MIX,
        "m: a stereo buffer is spatialised like a mono one",
        "    if channels >= 2 {\n        return (1.0, 1.0);\n    }",
        "    if channels >= 3 {\n        return (1.0, 1.0);\n    }",
        "KILLED",
    ),
    (
        MIX,
        "m: a command-built voice sounds before its Play arrives",
        "        v.playing = false;\n        self.voices.push((id, v));",
        "        self.voices.push((id, v));",
        "KILLED",
    ),
    (
        MIX,
        "m: a key-carrying attach is silently ignored rather than counted",
        "                C::AttachStaticBuffer(_) | C::AttachBufferStream(_, _) => self.ignored += 1,",
        "                C::AttachStaticBuffer(_) | C::AttachBufferStream(_, _) => {}",
        "KILLED",
    ),
    (
        MIX,
        "m: an underrun kills the voice instead of going silent",
        "                    } else if v.streaming {",
        "                    } else if false {",
        "KILLED",
    ),
    (
        MIX,
        "m: the final clamp is removed, so a dense scene runs past full scale",
        "        for s in out.iter_mut() {\n            *s = s.clamp(-1.0, 1.0);\n        }",
        "",
        "KILLED",
    ),
    (
        ENG,
        "m: the attenuation curve is inverse-distance rather than linear",
        "        let raw = 1.0\n            - ROLLOFF_FACTOR * (distance - REFERENCE_DISTANCE)\n                / (max_distance - REFERENCE_DISTANCE);",
        "        let raw = max_distance / (max_distance + distance);",
        "KILLED",
    ),
]

# Files whose test binaries may need reaping after a timeout.
STRAYS = ("rewo.exe", "rewo_net-*.exe", "rewo_audio-*.exe", "rewo_data-*.exe")


def reap():
    for exe in STRAYS:
        subprocess.run(["taskkill", "/F", "/IM", exe], capture_output=True)


def run_gate():
    """Build with `--features audio` and run `soundshot --check`.

    Returns "ok" / "failed" / "build". **The verdict is the exit code**, never a
    substring of the output: the gate prints ` ok ` on every passing witness
    line, so any grep for failure text is unreliable.
    """
    b = subprocess.run(
        ["cargo", "build", "-q", "-p", "rewo-app", "--features", "audio"],
        cwd=ROOT,
        capture_output=True,
        timeout=900,
    )
    if b.returncode != 0:
        return "build"
    exe = os.path.join(ROOT, "target", "debug", "rewo.exe")
    try:
        p = subprocess.run(
            [exe, "soundshot", "--check"], cwd=ROOT, capture_output=True, timeout=300
        )
    except subprocess.TimeoutExpired:
        # A hang is a KILL, not an outage — and the stray binary would hold the
        # link output and make the NEXT build fail with linker error 1104.
        reap()
        return "failed"
    return "ok" if p.returncode == 0 else "failed"


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 0
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    chosen = MUTATIONS[lo:hi]

    paths = sorted({m[0] for m in MUTATIONS})
    snapshots = {p: io.open(os.path.join(ROOT, p), "rb").read() for p in paths}

    # **Every anchor, before anything is written.** An anchor matching 0 or 2
    # times is a stale battery, not a pass, and finding that out half way
    # through means half the verdicts are already spent.
    stale = 0
    for rel, name, old, _new, _want in MUTATIONS:
        n = snapshots[rel].decode("utf-8").count(old)
        if n != 1:
            print("ANCHOR MATCHED %d TIMES: %s" % (n, name))
            stale += 1
    if stale:
        sys.exit("%d stale anchor(s) — nothing was run" % stale)

    print("BASELINE (unmutated, --features audio) ...", end=" ", flush=True)
    if run_gate() != "ok":
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for i, (rel, name, old, new, want) in enumerate(chosen, start=lo):
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        try:
            io.open(path, "wb").write(
                snapshot.decode("utf-8").replace(old, new).encode("utf-8")
            )
            r = run_gate()
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%3d %-64s %-10s (want %-9s) %s"
            % (i, name[:64], verdict, want, "ok" if ok else "<<< UNEXPECTED"),
            flush=True,
        )

    # The source is back; the BINARY is still the last mutant's. Rebuild, or the
    # next gate run in this tree grades a mutant against a clean checkout.
    print("rebuilding (the last mutation left a mutant binary) ...", flush=True)
    subprocess.run(
        ["cargo", "build", "-q", "-p", "rewo-app", "--features", "audio"],
        cwd=ROOT,
        capture_output=True,
    )

    # **Bytes, not `git diff --quiet`** — that cannot tell a leftover mutation
    # from ordinary uncommitted work, which is precisely the state a battery
    # runs in.
    leftover = [
        p for p in paths if io.open(os.path.join(ROOT, p), "rb").read() != snapshots[p]
    ]
    print("-----")
    print(
        "files restored: %s"
        % ("no -- MUTATION LEFT ON DISK: %s" % leftover if leftover else "yes")
    )
    print("%d/%d as expected" % (len(chosen) - bad, len(chosen)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
