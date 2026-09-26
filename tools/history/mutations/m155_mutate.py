"""M155's mutation battery — the tab list's hearts and faces.

Run: python tools/m155_mutate.py

Same discipline as m152/m154: a no-op control that must SURVIVE, exit codes
rather than substrings, a per-mutation timeout so a hang is a KILL rather than
an outage that leaves the mutant on disk, and a restore verified by BYTES.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TL = "crates/rewo-gpu/src/tab_list.rs"
VIEW = "crates/rewo-app/src/tab_list_view.rs"

MUTATIONS = [
    (
        "control: no change",
        TL,
        "pub const HEART_SPRITE_SIZE: i32 = 9;",
        "pub const HEART_SPRITE_SIZE: i32 = 9;",
        "rewo-gpu",
        "hearts_tests",
        "MUST SURVIVE — otherwise every KILLED below is vacuous",
    ),
    (
        "counts: both round the same way (ceil/ceil)",
        TL,
        "    let hearts_to_render = score.max(displayed.max(20)) / 2;",
        "    let hearts_to_render = positive_ceil_div(score.max(displayed.max(20)), 2);",
        "rewo-gpu",
        "hearts_tests",
        "the empty containers past the fill would be off by one at odd health",
    ),
    (
        "blink: the phase is measured from the START",
        TL,
        "self.blink_until_tick > tick && (self.blink_until_tick - tick).rem_euclid(6) >= 3",
        "self.blink_until_tick > tick && tick.rem_euclid(6) >= 3",
        "rewo-gpu",
        "hearts_tests",
        "damage and heal blinks would start on the wrong phase",
    ),
    (
        "blink: a decrease and an increase last the same",
        TL,
        """                + if value < self.last_value {
                    HEALTH_DECREASE_BLINK
                } else {
                    HEALTH_INCREASE_BLINK
                };""",
        "                + HEALTH_DECREASE_BLINK;",
        "rewo-gpu",
        "hearts_tests",
        "losing health is twice as loud as gaining it, and that would be lost",
    ),
    (
        "catch-up: the delay test is not strict",
        TL,
        "if tick - self.last_update_tick > HEALTH_DISPLAY_UPDATE_DELAY {",
        "if tick - self.last_update_tick >= HEALTH_DISPLAY_UPDATE_DELAY {",
        "rewo-gpu",
        "hearts_tests",
        "the ghost would clear one tick early",
    ),
    (
        "catch-up: update is an if/else rather than two halves",
        TL,
        """            self.last_value = value;
            self.last_update_tick = tick;
        }
        // Strictly greater""",
        """            self.last_value = value;
            self.last_update_tick = tick;
            return;
        }
        // Strictly greater""",
        "rewo-gpu",
        "hearts_tests",
        "EXPECTED SURVIVOR, proven equivalent: the branch above sets "
        "last_update_tick = tick, so the catch-up test reads 0 > 20 whenever the "
        "value changed. Pinned by a_change_never_catches_up_on_the_same_tick.",
    ),
    (
        "layering: a filled heart draws no container",
        TL,
        """        out.push(HeartBlit { dx: heart * per, sprite: container });
        if blink {""",
        "        if blink {",
        "rewo-gpu",
        "hearts_tests",
        "the empty half of a half-heart would be transparent",
    ),
    (
        "absorption: the tenth heart is ordinary",
        TL,
        """                sprite: if heart >= 10 {
                    HeartSprite::AbsorbingFull
                } else {
                    HeartSprite::Full
                },""",
        "                sprite: HeartSprite::Full,",
        "rewo-gpu",
        "hearts_tests",
        "gold absorption hearts would vanish",
    ),
    (
        "ghost: the blink layer reads the LIVE score",
        TL,
        "            if heart * 2 + 1 < displayed {",
        "            if heart * 2 + 1 < score {",
        "rewo-gpu",
        "hearts_tests",
        "the ghost would stop showing where the health used to be",
    ),
    (
        "zero health draws ten empty containers",
        TL,
        "    if full_hearts <= 0 {\n        return Vec::new();\n    }",
        "    if false {\n        return Vec::new();\n    }",
        "rewo-gpu",
        "hearts_tests",
        "a dead player would carry a row of empty hearts",
    ),
    (
        "faces: the flip is case insensitive",
        VIEW,
        'name == "Dinnerbone" || name == "Grumm"',
        'name.eq_ignore_ascii_case("Dinnerbone") || name.eq_ignore_ascii_case("Grumm")',
        "rewo-app",
        "face_tests",
        "a lower-case dinnerbone would be flipped where vanilla does not",
    ),
]


def run(crate, filt):
    flag = "--bins" if crate == "rewo-app" else "--lib"
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", crate, flag, filt],
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
    # One expected, proven-equivalent survivor (the two-halves coincidence).
    return 0 if killed >= total - 1 else 1


if __name__ == "__main__":
    sys.exit(main())
