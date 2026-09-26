"""M138c's mutation battery — the mixer.

    python tools/m138c_mutate.py

Every witness here reads the RENDERED OUTPUT rather than recomputing a gain, so
this battery is what says those readings are actually load-bearing. The two most
interesting entries are the pan ones: `right` is pitch-invariant, so most ways of
breaking the basis are invisible, and it took working out `right =
(-cos yaw, 0, -sin yaw)` on paper to find the one case that is not.

Same rules as its predecessors: verdicts from the EXIT CODE, a NO-OP CONTROL
that must SURVIVE, restore in a `finally`, and compare the touched file's BYTES
at the end rather than asking `git diff --quiet`.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
M = os.path.join("crates", "rewo-audio", "src", "mixer.rs")

MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// One sounding voice — a channel, in OpenAL's sense.",
        "/// One sounding voice, a channel in OpenAL's sense.",
        "SURVIVED",
    ),
    (
        "ignore distance attenuation entirely",
        "            let gain = v.gain * attenuation;",
        "            let gain = v.gain;",
        "KILLED",
    ),
    (
        "ignore the engine's own gain",
        "            let gain = v.gain * attenuation;",
        "            let gain = attenuation;",
        "KILLED",
    ),
    (
        "pan against FORWARD instead of RIGHT",
        "                ((rel[0] * right[0] + rel[1] * right[1] + rel[2] * right[2]) / distance)",
        "                ((rel[0] * self.listener.forward[0] + rel[1] * self.listener.forward[1] + rel[2] * self.listener.forward[2]) / distance)",
        "KILLED",
    ),
    (
        "mirror the stereo image (swap L and R)",
        "            let (mut l_gain, mut r_gain) = (angle.cos(), angle.sin());",
        "            let (mut l_gain, mut r_gain) = (angle.sin(), angle.cos());",
        "KILLED",
    ),
    (
        "treat every source as world-absolute (drop AL_SOURCE_RELATIVE)",
        "            let rel = if v.relative {",
        "            let rel = if false {",
        "KILLED",
    ),
    (
        "spatialise multi-channel sources too",
        "            if channels >= 2 {\n                l_gain = 1.0;\n                r_gain = 1.0;\n            }",
        "            if false {\n                l_gain = 1.0;\n                r_gain = 1.0;\n            }",
        "KILLED",
    ),
    (
        "pitch stops being a playback rate",
        "            let step = (v.pcm.sample_rate as f64 / self.out_rate as f64) * v.pitch as f64;",
        "            let step = v.pcm.sample_rate as f64 / self.out_rate as f64;",
        "KILLED",
    ),
    (
        "ignore the source rate (no resampling)",
        "            let step = (v.pcm.sample_rate as f64 / self.out_rate as f64) * v.pitch as f64;",
        "            let step = v.pitch as f64;",
        "KILLED",
    ),
    (
        "accumulate into the caller's buffer instead of clearing it",
        "        for s in out.iter_mut() {\n            *s = 0.0;\n        }\n        let frames = out.len() / 2;",
        "        let frames = out.len() / 2;",
        "KILLED",
    ),
    (
        "drop the output clamp",
        "        for s in out.iter_mut() {\n            *s = s.clamp(-1.0, 1.0);\n        }",
        "",
        "KILLED",
    ),
    (
        "a finished one-shot keeps sounding",
        "                        v.finished = true;",
        "                        v.cursor = 0.0;",
        "KILLED",
    ),
]


def run_tests():
    p = subprocess.run(
        ["cargo", "test", "-p", "rewo-audio", "--lib"], cwd=ROOT, capture_output=True
    )
    return p.returncode


def main():
    path = os.path.join(ROOT, M)
    snapshot = io.open(path, "rb").read()

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != 0:
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for name, old, new, want in MUTATIONS:
        text = snapshot.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-54s ANCHOR MATCHED %d TIMES" % (name[:54], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            verdict = "KILLED" if run_tests() != 0 else "SURVIVED"
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print("%-54s %-9s (want %-9s) %s" % (name[:54], verdict, want, "ok" if ok else "<<< UNEXPECTED"))

    leftover = io.open(path, "rb").read() != snapshot
    print("-----")
    print("file restored: %s" % ("no -- MUTATION LEFT ON DISK" if leftover else "yes"))
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
