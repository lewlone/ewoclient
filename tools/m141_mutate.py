"""M141's mutation battery — the ten tickable ramps, and M141a's f32 narrowing.

    python tools/m141_mutate.py

Same rules as its predecessors, with one correction: verdicts from the TEST
RESULT LINE rather than the exit code (see `run_tests` — reading the exit code
alone reported this battery's own no-op control as KILLED), a NO-OP CONTROL
that must SURVIVE, restore in a `finally`, a per-run timeout so a hang is a KILL
rather than an outage, and a byte comparison at the end rather than
`git diff --quiet` (which cannot tell a leftover mutation from uncommitted
work).

Unlike its predecessors this one mutates TWO files, because half the milestone
is the engine driving the ramps and half is the ramps themselves.

M141a's inline f32 narrowing has **no entry of its own**, and that is not an
omission: M141c moved it into `tickable::follow`, so the engine no longer
narrows anywhere and "the ramps' shared follow() drops the f32 narrowing" is
the same claim at its new single site. The battery reported it as
`ANCHOR MATCHED 0 TIMES` rather than silently passing, which is the behaviour
that made the move visible.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
T = os.path.join("crates", "rewo-net", "src", "tickable.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")

# (file, name, old, new, expected)
MUTATIONS = [
    (
        T,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// One instance's `tick()` body.",
        "/// One instance's per-tick body.",
        "SURVIVED",
    ),
    # --- the headline: the minecart's shadowed pitch ---------------------
    (
        T,
        "minecart writes the REAL pitch instead of the shadow",
        "        m.shadowed_pitch = mth_clamp(m.shadowed_pitch + 0.0025, 0.0, 1.0);",
        "        inst.pitch = mth_clamp(inst.pitch + 0.0025, 0.0, 1.0);",
        "KILLED",
    ),
    (
        T,
        "minecart shadow does not reset when the cart stalls",
        "        m.shadowed_pitch = 0.0;\n        inst.volume = 0.0;",
        "        inst.volume = 0.0;",
        "KILLED",
    ),
    # --- the two lerp-factor ceilings ------------------------------------
    (
        T,
        "bee volume factor clamped to 0..1 (ceiling becomes 1.2)",
        "        inst.volume = mth_lerp(mth_clamp(speed, 0.0, 0.5), 0.0, 1.2);",
        "        inst.volume = mth_lerp(mth_clamp(speed, 0.0, 1.0), 0.0, 1.2);",
        "KILLED",
    ),
    (
        T,
        "minecart volume factor clamped to 0..1 (ceiling becomes 0.7)",
        "        inst.volume = mth_lerp(mth_clamp(speed, 0.0, 0.5), 0.0, 0.7);",
        "        inst.volume = mth_lerp(mth_clamp(speed, 0.0, 1.0), 0.0, 0.7);",
        "KILLED",
    ),
    (
        T,
        "bee pitch factor clamped to 0..1 (the 'obvious fix')",
        "        inst.pitch = mth_lerp(mth_clamp(speed, min_pitch, max_pitch), min_pitch, max_pitch);",
        "        inst.pitch = mth_lerp(mth_clamp(speed, 0.0, 1.0), min_pitch, max_pitch);",
        "KILLED",
    ),
    (
        T,
        "bee pitch band ignores baby/adult",
        "    if baby {\n        (1.1, 1.5)\n    } else {\n        (0.7, 1.1)\n    }",
        "    let _ = baby;\n    (0.7, 1.1)",
        "KILLED",
    ),
    # --- clampedLerp is not lerp(clamp(..)) -------------------------------
    (
        T,
        "riding uses lerp(clamp(..)) instead of clampedLerp",
        "        r.volume_amplifier * mth_clamped_lerp(speed, r.volume_min, r.volume_max)",
        "        r.volume_amplifier * mth_lerp(mth_clamp(speed, r.volume_min, r.volume_max), r.volume_min, r.volume_max)",
        "KILLED",
    ),
    (
        T,
        "amplifier applied before the interpolation",
        "        r.volume_amplifier * mth_clamped_lerp(speed, r.volume_min, r.volume_max)",
        "        mth_clamped_lerp(speed * r.volume_amplifier, r.volume_min, r.volume_max)",
        "KILLED",
    ),
    # --- the riding overrides ---------------------------------------------
    (
        T,
        "minecart rider reads the VEHICLE's submersion, not the player's",
        "    let submerged = if r.is_minecart {\n        w.underwater(r.player)\n    } else {\n        w.underwater(r.vehicle)\n    };",
        "    let submerged = w.underwater(r.vehicle);",
        "KILLED",
    ),
    (
        T,
        "minecart rider reads the full speed, not the horizontal one",
        "    let speed = if r.is_minecart {\n        w.horizontal_speed(r.vehicle) as f32\n    } else {\n        w.speed(r.vehicle) as f32\n    };",
        "    let speed = w.speed(r.vehicle) as f32;",
        "KILLED",
    ),
    (
        T,
        "drop shoudlPlaySound's rails requirement entirely",
        "    let should_play = !r.is_minecart\n        || w.on_rails(r.vehicle)\n        || !w.new_minecart_behavior(r.vehicle);",
        "    let should_play = true;",
        "KILLED",
    ),
    (
        T,
        "riding does not check WHICH vehicle the player is on",
        "    if w.position(r.vehicle).is_none() || w.vehicle_of(r.player) != Some(r.vehicle) {",
        "    if w.position(r.vehicle).is_none() || w.vehicle_of(r.player).is_none() {",
        "KILLED",
    ),
    # --- the elytra --------------------------------------------------------
    (
        T,
        "elytra roots the squared speed",
        "    let speed = w.speed_sqr(e.player) as f32;",
        "    let speed = (w.speed_sqr(e.player) as f32).sqrt();",
        "KILLED",
    ),
    (
        T,
        "elytra survival guard is strict (< instead of <=)",
        "    if !(e.time <= ELYTRA_DELAY || w.fall_flying(e.player)) {",
        "    if !(e.time < ELYTRA_DELAY || w.fall_flying(e.player)) {",
        "KILLED",
    ),
    (
        T,
        "elytra pitch threshold moved off 0.8",
        "    inst.pitch = if inst.volume > 0.8 {\n        1.0 + (inst.volume - 0.8)",
        "    inst.pitch = if inst.volume > 0.5 {\n        1.0 + (inst.volume - 0.5)",
        "KILLED",
    ),
    (
        T,
        "elytra fade divided by 40 rather than 20",
        "        inst.volume *= (e.time - ELYTRA_DELAY) as f32 / ELYTRA_DELAY as f32;",
        "        inst.volume *= (e.time - ELYTRA_DELAY) as f32 / (2 * ELYTRA_DELAY) as f32;",
        "KILLED",
    ),
    # --- the guardian ------------------------------------------------------
    (
        T,
        "guardian volume linear rather than squared",
        "    inst.volume = 0.0 + 1.0 * scale * scale;",
        "    inst.volume = 0.0 + 1.0 * scale;",
        "KILLED",
    ),
    (
        T,
        "guardian ignores the (never-synced) AI target",
        "    if w.has_ai_target(guardian) {\n        return STOPPED;\n    }",
        "    if false {\n        return STOPPED;\n    }",
        "KILLED",
    ),
    (
        T,
        "elder guardian shares the ordinary attack duration",
        "pub const ELDER_GUARDIAN_ATTACK_DURATION: i32 = 60;",
        "pub const ELDER_GUARDIAN_ATTACK_DURATION: i32 = 80;",
        "KILLED",
    ),
    # --- the sniffer -------------------------------------------------------
    (
        T,
        "sniffer stop guard is a conjunction",
        "    if w.has_ai_target(sniffer) || !w.sniffer_digging(sniffer) {",
        "    if w.has_ai_target(sniffer) && !w.sniffer_digging(sniffer) {",
        "KILLED",
    ),
    # --- underwater --------------------------------------------------------
    (
        T,
        "underwater fade is symmetric (-1 instead of -2)",
        "        u.fade -= 2;",
        "        u.fade -= 1;",
        "KILLED",
    ),
    (
        T,
        "underwater guard read AFTER the step instead of before",
        "    if w.position(u.player).is_none() || u.fade < 0 {\n        return STOPPED;\n    }\n    if w.underwater(u.player) {",
        "    if w.position(u.player).is_none() {\n        return STOPPED;\n    }\n    if w.underwater(u.player) {",
        "KILLED",
    ),
    (
        T,
        "underwater fade capped below as well as above",
        "    u.fade = u.fade.min(UNDERWATER_FADE_DURATION);",
        "    u.fade = u.fade.clamp(0, UNDERWATER_FADE_DURATION);",
        "KILLED",
    ),
    # --- the biome loop's missing else -------------------------------------
    (
        T,
        "biome loop gains the `else` every other body has",
        "    let stopped = b.fade < 0;\n    b.fade += b.fade_direction;",
        "    if b.fade < 0 {\n        return STOPPED;\n    }\n    let stopped = false;\n    b.fade += b.fade_direction;",
        "KILLED",
    ),
    (
        T,
        "fadeIn caps instead of flooring",
        "        self.fade = self.fade.max(0);",
        "        self.fade = self.fade.min(0);",
        "KILLED",
    ),
    # --- the bee switch ----------------------------------------------------
    (
        T,
        "the bee does not stop on the tick it queues its replacement",
        "    if b.has_switched {\n        // Same tick: queued above, stopped here. The `else` branch is reached\n        // through `hasSwitched`, not through a second tick.\n        out.stopped = true;\n        return out;\n    }",
        "    if false {\n        out.stopped = true;\n        return out;\n    }",
        "KILLED",
    ),
    (
        T,
        "the bee switches on the wrong polarity",
        "        match self {\n            BeeLoop::Flying => angry,\n            BeeLoop::Aggressive => !angry,\n        }",
        "        match self {\n            BeeLoop::Flying => !angry,\n            BeeLoop::Aggressive => angry,\n        }",
        "KILLED",
    ),
    (
        T,
        "the replacement bee loop starts non-silent",
        "        volume: 0.0,\n        looping: true,",
        "        volume: 1.0,\n        looping: true,",
        "KILLED",
    ),
    # --- Mth ---------------------------------------------------------------
    (
        T,
        "isAngry drops the endTime > 0 test",
        "    anger_end_time > 0 && anger_end_time - game_time > 0",
        "    anger_end_time - game_time > 0",
        "KILLED",
    ),
    (
        T,
        "isAngry's deadline test is non-strict",
        "    anger_end_time > 0 && anger_end_time - game_time > 0",
        "    anger_end_time > 0 && anger_end_time - game_time >= 0",
        "KILLED",
    ),
    (
        T,
        "directionFromRotation drops the yaw half-turn",
        "    let y_cos = mth_cos(-rot_y * DEG - std::f32::consts::PI);\n    let y_sin = mth_sin(-rot_y * DEG - std::f32::consts::PI);",
        "    let y_cos = mth_cos(-rot_y * DEG);\n    let y_sin = mth_sin(-rot_y * DEG);",
        "KILLED",
    ),
    (
        T,
        "directionFromRotation drops the xCos negation",
        "    let x_cos = -mth_cos(-rot_x * DEG);",
        "    let x_cos = mth_cos(-rot_x * DEG);",
        "KILLED",
    ),
    (
        T,
        "the ramps' shared follow() drops the f32 narrowing",
        "    inst.x = pos.0 as f32 as f64;\n    inst.y = pos.1 as f32 as f64;\n    inst.z = pos.2 as f32 as f64;",
        "    inst.x = pos.0;\n    inst.y = pos.1;\n    inst.z = pos.2;",
        "KILLED",
    ),
    # --- M141c: the engine actually drives the ramps -----------------------
    (
        E,
        "M141c: a stopping tick still pushes volume/pitch/position",
        "                l.instance.looping = false;\n                to_stop.push(l.channel);\n                continue;",
        "                l.instance.looping = false;\n                to_stop.push(l.channel);",
        "KILLED",
    ),
    (
        E,
        "M141c: a ramp's queued replacement is dropped",
        "            if let Some((inst, next)) = outcome.queued {\n                queued.push((inst, Some(next)));\n            }",
        "            if let Some((inst, next)) = outcome.queued {\n                let _ = (inst, next);\n            }",
        "KILLED",
    ),
    (
        E,
        "M141c: the queued replacement loses its ramp",
        "                queued.push((inst, Some(next)));",
        "                let _ = next;\n                queued.push((inst, None));",
        "KILLED",
    ),
    # Both anchors below were retargeted by M141e, which replaced the inline
    # `match instance.binding` with `Ramp::for_instance` and the tick loop's
    # `ramp.entity()` with `ramp.silence_gated_entity()`. The battery reported
    # ANCHOR MATCHED 0 TIMES rather than passing quietly, which is how the move
    # became visible — the same behaviour that caught M141a's relocation.
    (
        E,
        "M141c: an entity-bound instance gets no ramp, so nothing ticks",
        "        let ramp = crate::tickable::Ramp::for_instance(&instance);",
        "        let ramp = None;",
        "KILLED",
    ),
    (
        E,
        "M141c: the silent-entity stop is skipped",
        "            if let Some(entity) = ramp.silence_gated_entity() {\n                if world.entity_silent(entity) {\n                    to_stop.push(l.channel);\n                }\n            }",
        "            if false {\n                to_stop.push(l.channel);\n            }",
        "KILLED",
    ),
]


def run_tests():
    """Returns "ok", "failed", or "build" — three outcomes, not two.

    **A battery that reads only the exit code cannot tell a failing test from a
    failing BUILD, and reports the thing it was built to find.** This one's
    no-op control came back KILLED on its first run, which is impossible for a
    comment edit; the cause was M138d's recorded hazard one step earlier in the
    sequence — the previous run's test binary still held the link output, so
    `cargo test` died with linker error 1104, exited non-zero, and read as a
    kill. Retrying once clears it; reporting it distinctly is what stops a
    future run from believing it.
    """
    for attempt in range(2):
        try:
            p = subprocess.run(
                ["cargo", "test", "-p", "rewo-net", "--lib"],
                cwd=ROOT,
                capture_output=True,
                timeout=300,
            )
        except subprocess.TimeoutExpired:
            # A hang is a KILL, not an outage — and the hung binary would
            # otherwise keep holding the link output and make the NEXT build
            # fail with linker error 1104, which reads as a broken tree.
            subprocess.run(
                ["taskkill", "/F", "/IM", "rewo_net-*.exe"], capture_output=True
            )
            return "failed"
        out = (p.stdout + p.stderr).decode("utf-8", "replace")
        if "test result: FAILED" in out:
            return "failed"
        if "test result: ok" in out and p.returncode == 0:
            return "ok"
        if attempt == 0:
            # Almost always the linker holding on to the previous binary.
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
            print("%-56s ANCHOR MATCHED %d TIMES" % (name[:56], n))
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
            "%-56s %-9s (want %-9s) %s"
            % (name[:56], verdict, want, "ok" if ok else "<<< UNEXPECTED")
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
