"""M170's mutation battery — the leash rope.

    python tools/m170_mutate.py [lo] [hi]

Every mutation names the CHECKER it claims coverage from (M158's gotcha). The
geometry (`leash::build_ribbon`) is graded by both the crate unit tests and the
GPU gate `rewo leashshot --check`; the pipeline/vertex-format live in the gate
only. `reap()` before every build (the linker-1104 hazard M169 hit); a timeout
is a KILL; restore verified by bytes; the no-op control must SURVIVE.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
LEASH = os.path.join(ROOT, "crates/rewo-gpu/src/leash.rs")

STRAYS = ("rewo.exe", "rewo_gpu-*.exe")

MUTATIONS = [
    ("control: no change", LEASH,
     "pub const LEASH_STEPS: i32 = 24;", "pub const LEASH_STEPS: i32 = 24;",
     ["gate", "unit"], "MUST SURVIVE — otherwise every verdict below is vacuous"),

    # ---- the ribbon geometry --------------------------------------------
    ("the slack curve is symmetric in dy", LEASH,
     "        if dy > 0.0 {\n            dy * progress * progress\n        } else {\n            dy - dy * (1.0 - progress) * (1.0 - progress)\n        }",
     "        dy * progress * progress",
     ["gate", "unit"], "an upward rope sags dy·p^2, a downward one dy - dy·(1-p)^2"),
    ("the slack curve is just linear", LEASH,
     "    let y = if slack {\n        if dy > 0.0 {\n            dy * progress * progress\n        } else {\n            dy - dy * (1.0 - progress) * (1.0 - progress)\n        }\n    } else {\n        dy * progress\n    };",
     "    let y = dy * progress;",
     ["gate", "unit"], "slack droops; taut is the straight interpolation"),
    ("the alternating dim never fires", LEASH,
     "    let color_modifier = if k % 2 == i32::from(backwards) { 0.7 } else { 1.0 };",
     "    let color_modifier = 1.0;",
     ["gate", "unit"], "k % 2 == backwards ? 0.7 : 1.0 twists the rope"),
    ("the light is not interpolated", LEASH,
     "        let light = start_light[c] + (end_light[c] - start_light[c]) * progress;",
     "        let light = start_light[c];",
     ["gate", "unit"], "Mth.lerp(progress, startLight, endLight) fades the rope"),
    ("the base tint is not brown", LEASH,
     "pub const LEASH_BASE_SRGB: [f32; 3] = [0.5, 0.4, 0.3];",
     "pub const LEASH_BASE_SRGB: [f32; 3] = [0.3, 0.4, 0.5];",
     ["gate", "unit"], "vanilla r=0.5 g=0.4 b=0.3 — r > g > b, a brown rope"),
    ("the ribbon width collapses", LEASH,
     "    let offset_factor = (inv_sqrt((dx * dx + dz * dz) as f64) * 0.05 / 2.0) as f32;",
     "    let offset_factor = 0.0;",
     ["gate", "unit"], "the perpendicular offset is the rope's whole visible thickness"),
    ("the second edge loses its fudge", LEASH,
     "        pos: [sx + x + dx_off, sy + y + 0.05 - fudge, sz + z - dz_off],",
     "        pos: [sx + x + dx_off, sy + y - fudge, sz + z - dz_off],",
     ["unit"], "the two edges sit 0.05 apart in y — a two-sided ribbon"),
    ("the strip is not expanded to triangles", LEASH,
     "    let mut out = Vec::with_capacity((strip.len() - 2) * 3);\n    for i in 0..strip.len() - 2 {\n        out.push(strip[i]);\n        out.push(strip[i + 1]);\n        out.push(strip[i + 2]);\n    }\n    out",
     "    strip.to_vec()",
     ["unit"], "a TRIANGLE_STRIP drawn as a LIST is garbage without expansion"),
    ("invSqrt drops its Newton step", LEASH,
     "    let mut y = f64::from_bits(l as u64);\n    y *= 1.5 - half * y * y;\n    y",
     "    f64::from_bits(l as u64)",
     ["unit"], "one Newton step brings the magic-bits estimate within 2e-3"),
]


def reap():
    for pat in STRAYS:
        subprocess.run(["taskkill", "/F", "/IM", pat], capture_output=True)


def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        reap()
        return None, ""
    return p.returncode, p.stdout + p.stderr


def build():
    reap()
    code, _ = run(["cargo", "build", "-p", "rewo-app"], 900)
    return code == 0


def unit():
    code, out = run(["cargo", "test", "-q", "-p", "rewo-gpu", "--lib", "leash"], 600)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "tests or build failed"
    if "test result:" not in out:
        return 1, "no test result line"
    return 0, "ok"


CHECKERS = {
    "gate": lambda: (lambda c: (c[0], "gate"))(run([EXE, "leashshot", "--check"], 300)),
    "unit": unit,
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in m[4]}) or ["unit"]
    print(f"[m170] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control; checkers {used}")
    results = []
    for name, path, find, repl, checkers, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        survived, reason = True, ""
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
            run_checkers = used if name.startswith("control") else checkers
            if not build():
                survived, reason = False, "build failed"
            else:
                for c in run_checkers:
                    code, r = CHECKERS[c]()
                    if code is None:
                        survived, reason = False, f"{c} TIMEOUT"
                        break
                    if code != 0:
                        survived, reason = False, f"{c} exit {code}"
                        break
                    reason = (reason + " " + f"{c} ok").strip()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path}"
            )
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})", flush=True)
        results.append((name, verdict, why))
    build()

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
    return 0 if killed == total else 1


if __name__ == "__main__":
    sys.exit(main())
