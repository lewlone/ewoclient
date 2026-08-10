"""M138b's mutation battery — the quantisation and the buffer library.

    python tools/m138b_mutate.py

The quantisation is the one part of the decode path with an exact vanilla
answer, and every way of getting it wrong is inaudible as a fault: a dropped
bias is a half-step offset, a floor instead of a truncation is a DC offset on
silence, a 32767 multiplier is one LSB at full scale. None of that announces
itself, so the literal vectors are the only thing holding it — this battery is
what says they hold it.

Same rules as the other harnesses: verdicts from the EXIT CODE; a NO-OP CONTROL
that must SURVIVE; restore in a `finally`, and compare the touched files' BYTES
at the end rather than asking `git diff --quiet`, which cannot tell a leftover
mutation from uncommitted work.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
Q = os.path.join("crates", "rewo-audio", "src", "quantise.rs")
B = os.path.join("crates", "rewo-audio", "src", "buffers.rs")

MUTATIONS = [
    (
        "CONTROL: a comment-only edit (must SURVIVE)",
        Q,
        "/// `Mth.clamp((int)(sample * 32767.5F - 0.5F), -32768, 32767)`.",
        "/// `Mth.clamp((int)(sample * 32767.5f - 0.5f), -32768, 32767)`.",
        "SURVIVED",
    ),
    (
        "multiplier 32767.5 -> 32767",
        Q,
        "    let scaled = sample * 32767.5 - 0.5;",
        "    let scaled = sample * 32767.0 - 0.5;",
        "KILLED",
    ),
    (
        "drop the -0.5 bias",
        Q,
        "    let scaled = sample * 32767.5 - 0.5;",
        "    let scaled = sample * 32767.5;",
        "KILLED",
    ),
    (
        "floor instead of truncating toward zero",
        Q,
        "    (scaled as i32).clamp(-32768, 32767) as i16",
        "    (scaled.floor() as i32).clamp(-32768, 32767) as i16",
        "KILLED",
    ),
    (
        "symmetric clamp -32767 instead of -32768",
        Q,
        "    (scaled as i32).clamp(-32768, 32767) as i16",
        "    (scaled as i32).clamp(-32767, 32767) as i16",
        "KILLED",
    ),
    (
        "buffer_size rounds DOWN to even",
        Q,
        "    (requested + 1) & !1",
        "    requested & !1",
        "KILLED",
    ),
    (
        "cache streams as well as statics",
        B,
        "    pub fn stream(&self, key: &str, looping: bool) -> StreamHandle {",
        "    pub fn stream(&mut self, key: &str, looping: bool) -> StreamHandle {\n        let _ = self.complete_buffer(key);",
        "KILLED",
    ),
    (
        "re-decode a static every time instead of caching it",
        B,
        "        if !self.cache.contains_key(key) {",
        "        if true {",
        "KILLED",
    ),
    (
        "retry a failed decode rather than caching the failure",
        B,
        "            let decoded = self.source.open(key);\n            self.cache.insert(key.to_string(), decoded);",
        "            let decoded = self.source.open(key);\n            if decoded.is_ok() {\n                self.cache.insert(key.to_string(), decoded);\n            } else {\n                self.cache.insert(key.to_string(), decoded);\n                self.cache.remove(key);\n                self.cache.insert(key.to_string(), self.source.open(key));\n            }",
        "KILLED",
    ),
    (
        "drop the loop flag from the stream handle",
        B,
        "        StreamHandle {\n            key: key.to_string(),\n            looping,\n        }",
        "        StreamHandle {\n            key: key.to_string(),\n            looping: false,\n        }",
        "KILLED",
    ),
]


def run_tests():
    p = subprocess.run(
        ["cargo", "test", "-p", "rewo-audio"], cwd=ROOT, capture_output=True
    )
    return p.returncode


def main():
    snapshots = {rel: io.open(os.path.join(ROOT, rel), "rb").read()
                 for _, rel, _, _, _ in MUTATIONS}

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != 0:
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for name, rel, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        original = io.open(path, "rb").read()
        text = original.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-52s ANCHOR MATCHED %d TIMES" % (name[:52], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            verdict = "KILLED" if run_tests() != 0 else "SURVIVED"
        finally:
            io.open(path, "wb").write(original)
        ok = verdict == want
        bad += 0 if ok else 1
        print("%-52s %-9s (want %-9s) %s" % (name[:52], verdict, want, "ok" if ok else "<<< UNEXPECTED"))

    leftover = [rel for rel, b in snapshots.items()
                if io.open(os.path.join(ROOT, rel), "rb").read() != b]
    print("-----")
    print("files restored: %s" % ("yes" if not leftover else "NO -- MUTATED: %s" % leftover))
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
