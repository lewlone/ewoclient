"""M149a's mutation battery — `EndFlashState`, the End's flash schedule.

    python tools/m149_mutate.py

Same rules as its predecessors: verdicts from the TEST RESULT LINE rather than
the exit code (reading the exit code alone cannot tell a failing test from a
failing build, and M141's own no-op control came back KILLED that way), a NO-OP
CONTROL that must SURVIVE, restore in a `finally`, a per-run timeout so a hang
is a KILL rather than an outage, and a byte comparison at the end rather than
`git diff --quiet` (which cannot tell a leftover mutation from uncommitted
work).

Two entries expect SURVIVED and are **proven equivalent** rather than left
looking untested; each says why in its own name. They are the reason this
battery is not simply "N/N killed".
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
F = os.path.join("crates", "rewo-world", "src", "end_flash.rs")

# (file, name, old, new, expected)
MUTATIONS = [
    (
        F,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `EndFlashState.tick(long)`.",
        "/// One tick of the End's flash schedule.",
        "SURVIVED",
    ),
    # --- the first interval, the headline ---------------------------------
    (
        F,
        "the seed guard is dropped, so interval 0 draws parameters",
        "        if new_seed == self.flash_seed {\n            return;\n        }",
        "        if false {\n            return;\n        }",
        "KILLED",
    ),
    (
        F,
        "flash_seed defaults to -1 ('never computed') instead of Java's 0",
        "#[derive(Clone, Copy, Debug, Default, PartialEq)]\npub struct EndFlashState {",
        "impl Default for EndFlashState {\n"
        "    fn default() -> Self {\n"
        "        Self {\n"
        "            flash_seed: -1,\n"
        "            offset: 0,\n"
        "            duration: 0,\n"
        "            intensity: 0.0,\n"
        "            old_intensity: 0.0,\n"
        "            x_angle: 0.0,\n"
        "            y_angle: 0.0,\n"
        "        }\n"
        "    }\n"
        "}\n"
        "#[derive(Clone, Copy, Debug, PartialEq)]\npub struct EndFlashState {",
        "KILLED",
    ),
    # --- the RNG composition ----------------------------------------------
    (
        F,
        "the discarded nextFloat() is removed",
        "        let _ = random.next_float();",
        "",
        "KILLED",
    ),
    (
        F,
        "offset and duration are drawn in the other order",
        "        self.offset = random_between_inclusive(&mut random, 0, MAX_FLASH_OFFSET_IN_TICKS);\n"
        "        self.duration = random_between_inclusive(\n"
        "            &mut random,\n"
        "            MIN_FLASH_DURATION_IN_TICKS,\n"
        "            MAX_FLASH_DURATION_IN_TICKS\n"
        "                .min(FLASH_INTERVAL_IN_TICKS as i32 - self.offset),\n"
        "        );",
        "        self.duration = random_between_inclusive(\n"
        "            &mut random,\n"
        "            MIN_FLASH_DURATION_IN_TICKS,\n"
        "            MAX_FLASH_DURATION_IN_TICKS\n"
        "                .min(FLASH_INTERVAL_IN_TICKS as i32 - self.offset),\n"
        "        );\n"
        "        self.offset = random_between_inclusive(&mut random, 0, MAX_FLASH_OFFSET_IN_TICKS);",
        "KILLED",
    ),
    (
        F,
        "x_angle and y_angle draws are swapped",
        "        self.x_angle = random_between(&mut random, -60.0, 10.0);\n        self.y_angle = random_between(&mut random, -180.0, 180.0);",
        "        self.y_angle = random_between(&mut random, -180.0, 180.0);\n        self.x_angle = random_between(&mut random, -60.0, 10.0);",
        "KILLED",
    ),
    (
        F,
        "randomBetweenInclusive loses its +1 (becomes exclusive)",
        "    random.next_int(max_inclusive - min + 1) + min",
        "    random.next_int(max_inclusive - min) + min",
        "KILLED",
    ),
    (
        F,
        "randomBetween scales by max instead of the span",
        "    random.next_float() * (max_exclusive - min) + min",
        "    random.next_float() * max_exclusive + min",
        "KILLED",
    ),
    (
        F,
        "the interval length changes, so every seed changes",
        "pub const FLASH_INTERVAL_IN_TICKS: i64 = 600;",
        "pub const FLASH_INTERVAL_IN_TICKS: i64 = 601;",
        "KILLED",
    ),
    (
        F,
        "the offset bound changes",
        "pub const MAX_FLASH_OFFSET_IN_TICKS: i32 = 200;",
        "pub const MAX_FLASH_OFFSET_IN_TICKS: i32 = 199;",
        "KILLED",
    ),
    # --- the tick sequence -------------------------------------------------
    (
        F,
        "old_intensity captured AFTER the new value (edge never fires)",
        "        self.old_intensity = self.intensity;\n        self.intensity = self.calculate_intensity(clock_time);",
        "        self.intensity = self.calculate_intensity(clock_time);\n        self.old_intensity = self.intensity;",
        "KILLED",
    ),
    (
        F,
        "rising edge tests old_intensity < 0.0 instead of <= 0.0",
        "        self.intensity > 0.0 && self.old_intensity <= 0.0",
        "        self.intensity > 0.0 && self.old_intensity < 0.0",
        "KILLED",
    ),
    (
        F,
        "rising edge tests intensity >= 0.0 instead of > 0.0",
        "        self.intensity > 0.0 && self.old_intensity <= 0.0",
        "        self.intensity >= 0.0 && self.old_intensity <= 0.0",
        "KILLED",
    ),
    # --- the window and the curve -----------------------------------------
    (
        F,
        "the window's upper bound becomes exclusive",
        "            && within <= i64::from(self.offset) + i64::from(self.duration)",
        "            && within < i64::from(self.offset) + i64::from(self.duration)",
        "KILLED",
    ),
    (
        F,
        "EQUIVALENT: the window's lower bound becomes exclusive (sin(0) is 0 either way)",
        "        if within >= i64::from(self.offset)",
        "        if within > i64::from(self.offset)",
        "SURVIVED",
    ),
    (
        F,
        "the sine expression is regrouped as t * (PI / duration)",
        "                (within - i64::from(self.offset)) as f32 * std::f32::consts::PI\n                    / self.duration as f32,",
        "                (within - i64::from(self.offset)) as f32\n                    * (std::f32::consts::PI / self.duration as f32),",
        "KILLED",
    ),
    (
        F,
        "Mth's sine table is replaced by the platform's sin",
        "            mth_sin(\n                (within - i64::from(self.offset)) as f32 * std::f32::consts::PI",
        "            f32::sin(\n                (within - i64::from(self.offset)) as f32 * std::f32::consts::PI",
        "KILLED",
    ),
    # --- the clamp and the divide ------------------------------------------
    (
        F,
        "EQUIVALENT: the inert Math.min(380, 600 - offset) is folded away",
        "            MAX_FLASH_DURATION_IN_TICKS\n                .min(FLASH_INTERVAL_IN_TICKS as i32 - self.offset),",
        "            MAX_FLASH_DURATION_IN_TICKS,",
        "SURVIVED",
    ),
    (
        F,
        "the seed divide becomes Euclidean (differs for a negative clock)",
        "        let new_seed = clock_time / FLASH_INTERVAL_IN_TICKS;",
        "        let new_seed = clock_time.div_euclid(FLASH_INTERVAL_IN_TICKS);",
        "KILLED",
    ),
    (
        F,
        "the within-interval remainder becomes Euclidean",
        "        let within = clock_time % FLASH_INTERVAL_IN_TICKS;",
        "        let within = clock_time.rem_euclid(FLASH_INTERVAL_IN_TICKS);",
        "KILLED",
    ),
    # --- the lerp ----------------------------------------------------------
    (
        F,
        "getIntensity lerps from new to old (endpoints swapped)",
        "        self.old_intensity + partial_ticks * (self.intensity - self.old_intensity)",
        "        self.intensity + partial_ticks * (self.old_intensity - self.intensity)",
        "KILLED",
    ),
    (
        F,
        "getIntensity ignores the partial and returns this tick",
        "        self.old_intensity + partial_ticks * (self.intensity - self.old_intensity)",
        "        self.intensity",
        "KILLED",
    ),
]


def run_tests():
    """Returns "ok", "failed", or "build" — three outcomes, not two.

    A battery that reads only the exit code cannot tell a failing test from a
    failing BUILD, and so reports the thing it was built to find. Retrying once
    clears the common case (the previous run's test binary still holding the
    link output, which surfaces as linker error 1104).
    """
    for attempt in range(2):
        try:
            p = subprocess.run(
                ["cargo", "test", "-p", "rewo-world", "--lib"],
                cwd=ROOT,
                capture_output=True,
                timeout=420,
            )
        except subprocess.TimeoutExpired:
            # A hang is a KILL, not an outage — and the hung binary would
            # otherwise keep holding the link output and make the NEXT build
            # fail with linker error 1104, which reads as a broken tree.
            subprocess.run(
                ["taskkill", "/F", "/IM", "rewo_world-*.exe"], capture_output=True
            )
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
        # Every `crates/rewo-*` source is CRLF (`core.autocrlf` is false, and
        # there is no `.gitattributes`), while the anchors below are written
        # with `\n`. Match against an LF-normalised copy and re-encode on the
        # way out, or every multi-line anchor reports MATCHED 0 TIMES — which
        # reads as a stale battery rather than as an encoding mismatch.
        crlf = b"\r\n" in snapshot
        text = snapshot.decode("utf-8").replace("\r\n", "\n")
        n = text.count(old)
        if n != 1:
            print("%-62s ANCHOR MATCHED %d TIMES" % (name[:62], n))
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
            "%-62s %-10s (want %-10s) %s"
            % (name[:62], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

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
