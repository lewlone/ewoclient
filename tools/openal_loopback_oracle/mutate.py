"""M139's mutation battery - are the loopback oracle's witnesses load-bearing?

    python tools/openal_loopback_oracle/mutate.py

Runs ONLY `mixer::tests::oracle`, deliberately. The rest of `rewo-audio`'s
suite would kill most of these on its own, and that would answer a different
question: what this battery asks is whether the fourteen tests M139 added see
the things they claim to measure. A mutation that survives here and dies in the
older suite is still a gap in M139.

Same rules as the other batteries in `tools/`: verdicts read from the TEST
RESULT LINE rather than the exit code (which cannot tell a failing test from a
failing BUILD), a NO-OP CONTROL that must SURVIVE, restore in a `finally`, a
per-run timeout so a hang is a KILL rather than an outage that leaves the
mutation on disk, a stray-process reaper (a hung test binary holds the link
output and the next build fails with linker error 1104, looking like a broken
tree), and a byte comparison at the end rather than `git diff --quiet`, which
cannot tell a leftover mutation from uncommitted work.

Two entries expect SURVIVED and say why in their own names.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
M = os.path.join("crates", "rewo-audio", "src", "mixer.rs")

TESTS = ["cargo", "test", "-q", "-p", "rewo-audio", "--lib", "mixer::tests::oracle"]

# (name, old, new, expected)
MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// Equal-power pan, and OpenAL's refusal to spatialise a multi-channel buffer.",
        "/// The pan law, and multi-channel buffers.",
        "SURVIVED",
    ),
    # --- the pan law -------------------------------------------------------
    (
        "pan_gains swaps its channels, so every image is mirrored",
        "    (angle.cos(), angle.sin())",
        "    (angle.sin(), angle.cos())",
        "KILLED",
    ),
    (
        "pan_gains goes linear instead of equal-power",
        "    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;\n    (angle.cos(), angle.sin())",
        "    let t = (pan + 1.0) * 0.5;\n    (1.0 - t, t)",
        "KILLED",
    ),
    (
        "a stereo buffer gets spatialised after all",
        "    if channels >= 2 {\n        return (1.0, 1.0);\n    }",
        "",
        "KILLED",
    ),
    # --- the listener basis ------------------------------------------------
    (
        "the right vector is negated, which is the INITIAL-vs-yaw-0 half turn",
        "        let right = [right[0] as f32, right[1] as f32, right[2] as f32];",
        "        let right = [-right[0] as f32, -right[1] as f32, -right[2] as f32];",
        "KILLED",
    ),
    (
        "the up vector is pinned to +Y, so right() degenerates at pitch +-90",
        "        let right = self.listener.right();",
        "        let right = ListenerTransform { up: [0.0, 1.0, 0.0], ..self.listener }.right();",
        "KILLED",
    ),
    # --- gain and distance -------------------------------------------------
    (
        "attenuation is dropped from the gain",
        "            let gain = v.gain * attenuation;",
        "            let gain = v.gain;",
        "KILLED",
    ),
    (
        "the linear curve becomes inverse-square, which is right at 0 and nowhere else",
        "                Some(max) => openal::linear_gain(distance, max),",
        "                Some(max) => 1.0 / (1.0 + distance / max).powi(2),",
        "KILLED",
    ),
    # --- AL_SOURCE_RELATIVE ------------------------------------------------
    (
        "relative stops skipping the listener-position subtraction",
        "            let rel = if v.relative {\n                v.position\n            } else {",
        "            let rel = if false {\n                v.position\n            } else {",
        "KILLED",
    ),
    # --- the resampler -----------------------------------------------------
    (
        "sample_at drops interpolation and takes the nearest sample",
        "    let l = get(i, 0) * (1.0 - frac) + get(j, 0) * frac;",
        "    let l = get(i, 0);",
        "KILLED",
    ),
    (
        "the resampling step ignores pitch",
        "        let step = (v.pcm.sample_rate as f64 / self.out_rate as f64) * v.pitch as f64;",
        "        let step = v.pcm.sample_rate as f64 / self.out_rate as f64;",
        "KILLED",
    ),
    # --- the output stage --------------------------------------------------
    (
        "the hard clamp is removed, so 32 coherent voices run off the scale",
        "        for s in out.iter_mut() {\n            *s = s.clamp(-1.0, 1.0);\n        }",
        "",
        "KILLED",
    ),
    # --- the consumer's own instrument -------------------------------------
    (
        "the vector parser reads rmsR where rmsL belongs",
        "                    rms_l: n(23),\n                    rms_r: n(24),",
        "                    rms_l: n(24),\n                    rms_r: n(23),",
        "KILLED",
    ),
    (
        "the stimulus is regenerated at the wrong amplitude",
        "                let l = (r.amp_l * (2.0 * std::f64::consts::PI * r.freq_l * t / r.srate as f64).sin())",
        "                let l = (0.999 * r.amp_l * (2.0 * std::f64::consts::PI * r.freq_l * t / r.srate as f64).sin())",
        "KILLED",
    ),
    (
        "the consumer measures the WARM-UP window instead of the measured one",
        "            sink.rendered[WARMUP_FRAMES * 2..].to_vec()",
        "            sink.rendered[..MEASURE_FRAMES * 2].to_vec()",
        "SURVIVED: a steady looping tone is stationary, so the window's offset "
        "cannot matter. Recorded rather than removed because the offset is what "
        "keeps the two renderers aligned if a stimulus is ever made transient.",
    ),
    (
        "the consumer's DFT window drops to Hann",
        "                let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()\n                    - 0.01168 * (3.0 * t).cos();",
        "                let w = 0.5 - 0.5 * t.cos();",
        "KILLED",
    ),
    (
        "the consumer's fundamental band narrows below the window's main lobe",
        "        const FUND_HALFWIDTH: i64 = 6;",
        "        const FUND_HALFWIDTH: i64 = 2;",
        "KILLED",
    ),
]

# Entries whose `want` carries an explanation after a colon.
def want_of(entry):
    return entry[3].split(":")[0]


def reap():
    for image in ("rewo_audio-*.exe", "rewo-*.exe"):
        subprocess.run(["taskkill", "/F", "/IM", image], capture_output=True)


def run_tests():
    """Returns "ok", "failed", or "build" - three outcomes, not two."""
    for attempt in range(2):
        try:
            p = subprocess.run(TESTS, cwd=ROOT, capture_output=True, timeout=600)
        except subprocess.TimeoutExpired:
            reap()
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
    path = os.path.join(ROOT, M)
    snapshot = io.open(path, "rb").read()
    crlf = b"\r\n" in snapshot
    text = snapshot.decode("utf-8").replace("\r\n", "\n")

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != "ok":
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for entry in MUTATIONS:
        name, old, new = entry[0], entry[1], entry[2]
        want = want_of(entry)
        n = text.count(old)
        if n != 1:
            print("%-64s ANCHOR MATCHED %d TIMES" % (name[:64], n))
            bad += 1
            continue
        try:
            mutated = text.replace(old, new)
            if crlf:
                mutated = mutated.replace("\n", "\r\n")
            io.open(path, "wb").write(mutated.encode("utf-8"))
            r = run_tests()
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-64s %-10s (want %-10s) %s"
            % (name[:64], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

    leftover = io.open(path, "rb").read() != snapshot
    print("-----")
    print("file restored: %s" % ("no -- MUTATION LEFT ON DISK" if leftover else "yes"))
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
