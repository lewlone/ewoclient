"""M169's mutation battery — the jump bar's inputs.

    python tools/m169_mutate.py [lo] [hi]

Each mutation names the CHECKER it is claiming coverage from (M158's gotcha).
The jump meter, the selector, the packet and the dash cooldown are all graded
by `rewo gaugeshot --check` (its j-witnesses) and the crate unit tests; the
saddle decode and the metadata arms by the crate tests. The live r59 chain is
not mutated here — it needs a server and is a separate ~90 s run per mutation.

Gate-routed, so the tree is rebuilt before the check and after the restore
(m164's finding). A timeout is a KILL and reaps stray binaries first. The
no-op control must SURVIVE. Exit codes, not substrings; restore verified by
bytes; `lo`/`hi` slices stay inside the tool cap.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
JR = os.path.join(ROOT, "crates/rewo-net/src/jump_riding.rs")
LB = os.path.join(ROOT, "crates/rewo-gpu/src/locator_bar.rs")
ENT = os.path.join(ROOT, "crates/rewo-world/src/entities.rs")
META = os.path.join(ROOT, "crates/rewo-net/src/metadata.rs")
LIB = os.path.join(ROOT, "crates/rewo-net/src/lib.rs")
SH = os.path.join(ROOT, "crates/rewo-gpu/src/survival_hud.rs")

STRAYS = ("rewo.exe", "rewo_gpu-*.exe", "rewo_net-*.exe", "rewo_world-*.exe", "rewo_data-*.exe")

MUTATIONS = [
    ("control: no change", JR,
     "pub const START_RIDING_JUMP: i32 = 3;", "pub const START_RIDING_JUMP: i32 = 3;",
     ["gate", "unit-net", "unit-gpu", "unit-world"],
     "MUST SURVIVE — otherwise every verdict below is vacuous"),

    # ---- the meter -------------------------------------------------------
    ("the ramp has a 1.0 cap after ten ticks", JR,
     "                        self.scale = 0.8 + 2.0 / (self.ticks - 9) as f32 * 0.1;",
     "                        self.scale = 1.0;",
     ["gate", "unit-net"], "there is no cap; tick 11 is 0.9, not 1.0"),
    ("the press does not zero the scale", JR,
     "                } else if !was_jumping && jumping {\n                    self.ticks = 0;\n                    self.scale = 0.0;",
     "                } else if !was_jumping && jumping {\n                    self.ticks = 0;",
     ["unit-net"], "`ticks = 0, scale = 0` on the rising edge"),
    ("the release does not send", JR,
     "                if was_jumping && !jumping {\n                    self.ticks = -10;\n                    send = Some(riding_jump_data(self.scale));",
     "                if was_jumping && !jumping {\n                    self.ticks = -10;",
     ["gate", "unit-net"], "the release is the only thing that sends START_RIDING_JUMP"),
    ("the release zeroes the scale immediately", JR,
     "                if was_jumping && !jumping {\n                    self.ticks = -10;\n                    send = Some(riding_jump_data(self.scale));",
     "                if was_jumping && !jumping {\n                    self.ticks = -10;\n                    self.scale = 0.0;\n                    send = Some(riding_jump_data(self.scale));",
     ["unit-net"], "the bar holds full for ten ticks; the -10 park is what zeroes it"),
    ("a cooldown does not block the meter", JR,
     "            Some(v) if v.cooldown == 0 => {",
     "            Some(_v) => {",
     ["unit-net"], "`getJumpCooldown() == 0` is the guard; a dash cooldown is the else"),
    ("the packet data rounds instead of flooring", JR,
     "    (scale * 100.0).floor() as i32",
     "    (scale * 100.0).round() as i32",
     ["gate", "unit-net"], "`Mth.floor(scale * 100)`"),
    ("the action ordinal is wrong", JR,
     "pub const START_RIDING_JUMP: i32 = 3;", "pub const START_RIDING_JUMP: i32 = 4;",
     ["gate", "unit-net"], "the enum's FOURTH constant is ordinal 3"),

    # ---- the selector ----------------------------------------------------
    ("the vehicle needs a scale to win without waypoints", LB,
     "    } else if jumpable.is_some() {\n        ContextualInfo::JumpableVehicle\n",
     "    } else if prioritise_jump {\n        ContextualInfo::JumpableVehicle\n",
     ["gate", "unit-gpu"], "without waypoints a jumpable vehicle ALWAYS wins, scale 0 or not"),
    ("the vehicle wins over the locator idle", LB,
     "        if jumpable.is_some() && prioritise_jump {",
     "        if jumpable.is_some() {",
     ["gate", "unit-gpu"], "with waypoints it wins only while scale > 0 || cooldown > 0"),

    # ---- the vehicle state ----------------------------------------------
    ("the dash cooldown restarts on every DASH", ENT,
     "            if e.ticked && e.dash_cooldown == 0 {\n                e.dash_cooldown = ticks;\n            }",
     "            if e.ticked {\n                e.dash_cooldown = ticks;\n            }",
     ["gate", "unit-world"], "`dashCooldown == 0 ? 55 : dashCooldown` does NOT restart a running one"),
    ("the dash arms before the first tick", ENT,
     "            if e.ticked && e.dash_cooldown == 0 {",
     "            if e.dash_cooldown == 0 {",
     ["gate", "unit-world"], "`!this.firstTick` — the spawn-time DASH entry arms nothing"),
    ("a positive pose tick is sitting", ENT,
     "        let sitting = e.last_pose_change_tick < 0;",
     "        let sitting = e.last_pose_change_tick > 0;",
     ["unit-world"], "`isCamelSitting()` is `LAST_POSE_CHANGE_TICK < 0`"),
    ("the standing transition window is the sitting one", ENT,
     "        let in_transition = pose_time < if sitting { 40 } else { 52 };",
     "        let in_transition = pose_time < if sitting { 40 } else { 40 };",
     ["unit-world"], "40 sitting, 52 standing"),

    # ---- the wire --------------------------------------------------------
    ("the saddle slot is not read", LIB,
     "        if slot_id & 127 == 7 {\n            entities.set_saddled(eid, !matches!(slot, WireSlot::Empty));\n        }",
     "        if slot_id & 127 == 99 {\n            entities.set_saddled(eid, !matches!(slot, WireSlot::Empty));\n        }",
     ["unit-net"], "SADDLE is EquipmentSlot ordinal 7 — the whole of isSaddled(); the cow test in `rewo-net` drives the wire path, not the gate"),
    ("the camel DASH arms the wrong literal", LIB,
     "    if meta.bool19.is_some() && kinds.classes.is_some_and(|c| c.is_camel(type_id)) {\n        entities.arm_dash_cooldown(eid, 55);",
     "    if meta.bool19.is_some() && kinds.classes.is_some_and(|c| c.is_camel(type_id)) {\n        entities.arm_dash_cooldown(eid, 40);",
     ["gate"], "the camel's dashCooldown is 55; the nautilus's is 40"),
    ("the dash index is the anger index", META,
     "            (19, 8) => meta.bool19 = r.u8().ok().map(|b| b != 0),",
     "            (18, 8) => meta.bool19 = r.u8().ok().map(|b| b != 0),",
     ["gate"], "Camel.DASH is the camel's first own accessor, index 19"),

    # ---- the layout ------------------------------------------------------
    ("the jump progress is not a sub-rectangle", SH),
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
    reap()  # test binaries from a prior `cargo test` hold the exe (linker 1104)
    code, _ = run(["cargo", "build", "-p", "rewo-app"], 900)
    return code == 0


def unit(crate):
    code, out = run(["cargo", "test", "-q", "-p", crate, "--lib"], 600)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "tests or build failed"
    if "test result:" not in out:
        return 1, "no test result line"
    return 0, "ok"


CHECKERS = {
    "gate": lambda: (lambda c: (c[0], "gate"))(run([EXE, "gaugeshot", "--check"], 300)),
    "unit-net": lambda: unit("rewo-net"),
    "unit-gpu": lambda: unit("rewo-gpu"),
    "unit-world": lambda: unit("rewo-world"),
}

# The last row is a placeholder to keep the `.floor()` jump-progress mutation
# honest; fill it in code so the anchor is exact.
MUTATIONS[-1] = (
    "the jump progress ignores the -1", SH,
    "    P0 + (scale * (delta - 1) as f32).floor() as i32 + i32::from(scale > 0.0)",
    "    P0 + (scale * delta as f32).floor() as i32 + i32::from(scale > 0.0)",
    ["gate"], "`lerpDiscrete(alpha, 0, 182)` multiplies by `delta - 1`, not `delta`",
)


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in m[4]}) or ["gate"]
    print(f"[m169] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control; checkers {used}")
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
                f"RESTORE FAILED for {path} — mutation may be left on disk"
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
