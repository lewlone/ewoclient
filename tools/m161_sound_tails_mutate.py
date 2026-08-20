"""M161's mutation battery — the sound tails of `explode` and `level_event`.

Run:  python tools/m161_sound_tails_mutate.py [lo] [hi]

**The filename carries the task, not just the M-number.** Three branches of the
same parallel wave each created `tools/m161_mutate.py` with different contents —
an add/add conflict at one path, and the M-number triple-claimed. The number is
a merge-silent resource in the sense REWO_PLAN section 0.0 describes; a
task-suffixed name costs nothing and removes the collision.

**Two routes, deliberately.** The milestone ships both a gate (`soundshot`'s
w9-w16) and unit tests, and gotcha 0d says a battery has to be routed through
whatever it claims coverage from: M155's ran entirely through `cargo test` and
so never asked whether its gate could reach the code at all. Each mutation below
declares its own route, and several are here specifically to check that the
GATE — not only the unit suite — sees the break. Four entries appear TWICE, once
per route, because the review's headline defect was precisely a break that one
route could see and the other could not.

Discipline per AGENT_LOOP_BRIEF and REWO_PLAN section 0.0:

  * a no-op control that must SURVIVE, or every KILLED below is vacuous;
  * a BASELINE check before anything is touched, because a battery run against
    an already-red command reads KILLED for every entry (M109 lost two whole
    batteries to that);
  * verdicts from EXIT CODES plus the command's own summary line, never a
    substring of the body — a panic must not read as a pass (M85);
  * a per-mutation timeout, so a hang is a KILL rather than an outage whose
    `finally` never runs and leaves the mutant on disk (M104);
  * a REBUILD after every restore, since a gate-routed battery otherwise grades
    the previous mutant's BINARY against a clean tree (M158);
  * the restore verified by BYTES, not by `git diff`, which cannot tell a
    leftover mutation from uncommitted work (M138a).

Sliceable, because the gate-routed half costs a rebuild per mutation and the
whole battery does not fit inside the 10-minute tool cap. The control is
prepended to every slice, so no slice can report a verdict without its own
validity check.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIB = os.path.join(ROOT, "crates/rewo-net/src/lib.rs")
MOTION = os.path.join(ROOT, "crates/rewo-net/src/motion.rs")
POPT = os.path.join(ROOT, "crates/rewo-net/src/particle_options.rs")
ENGINE = os.path.join(ROOT, "crates/rewo-net/src/sound_engine.rs")
PLAY = os.path.join(ROOT, "crates/rewo-net/src/play.rs")
NOISE = os.path.join(ROOT, "crates/rewo-world/src/biome_noise.rs")
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")

# route: "gate" -> `rewo soundshot --check`;  "net"/"world" -> `cargo test -p …`
MUTATIONS = [
    (
        "control: no change",
        "gate",
        LIB,
        "    if distance_to_sqr <= 100.0 {",
        "    if distance_to_sqr <= 100.0 {",
        "MUST SURVIVE - otherwise every verdict below is vacuous",
    ),
    # ---- the bearing (w9) ------------------------------------------------
    (
        "bearing: the subtraction runs camera - target",
        "gate",
        LIB,
        """    let d = [
        target[0] - camera[0],
        target[1] - camera[1],
        target[2] - camera[2],
    ];""",
        """    let d = [
        camera[0] - target[0],
        camera[1] - target[1],
        camera[2] - target[2],
    ];""",
        "a perfect 180 degree inversion: the wither's roar on the wrong side of "
        "your head, which a distance-only witness passes",
    ),
    (
        "bearing: scaled by 1.0 rather than 2.0",
        "gate",
        LIB,
        """    [
        camera[0] + dir[0] * 2.0,
        camera[1] + dir[1] * 2.0,
        camera[2] + dir[2] * 2.0,
    ]""",
        """    [
        camera[0] + dir[0] * 1.0,
        camera[1] + dir[1] * 1.0,
        camera[2] + dir[2] * 1.0,
    ]""",
        "the sound would sit one block from the listener rather than two",
    ),
    (
        "bearing: the camera-placed output takes the block path's half-block",
        "gate",
        LIB,
        "        Placement::Camera => camera_bearing_position(camera?, (x, y, z)),",
        """        Placement::Camera => {
            let p = camera_bearing_position(camera?, (x, y, z));
            [p[0] + 0.5, p[1] + 0.5, p[2] + 0.5]
        }""",
        "a second, invisible half-block error on top of a correct one - the "
        "centring belongs to the BlockPos overload, not to this one",
    ),
    (
        "camera: an absent one falls back to the origin instead of silence",
        "gate",
        LIB,
        "        Placement::Camera => camera_bearing_position(camera?, (x, y, z)),",
        "        Placement::Camera => camera_bearing_position(camera.unwrap_or([0.0; 3]), (x, y, z)),",
        "a wither spawn heard before the camera exists would play two blocks "
        "from (0,0,0); `LevelEventHandler.java:66` has no else",
    ),
    # ---- the listener row (w11) ------------------------------------------
    (
        "1032: pitch and volume swapped at the call",
        "gate",
        LIB,
        "            crate::sound_instance::SoundInstance::for_local_ambience(row.sound, 1.0, volume),",
        "            crate::sound_instance::SoundInstance::for_local_ambience(row.sound, volume, 1.0),",
        "a portal four times too loud at a fixed pitch - forLocalAmbience takes "
        "PITCH second",
    ),
    (
        "1032: emitted as a positioned sound at the block",
        "gate",
        LIB,
        "    if row.placement == Placement::Listener {",
        "    if false {",
        "the portal would pan and attenuate where vanilla's does neither",
    ),
    # ---- the distance delay (w12) ----------------------------------------
    (
        "delay: divided by 340 rather than 40",
        "gate",
        LIB,
        "    let delay_in_seconds = distance_to_sqr.sqrt() / 40.0;",
        "    let delay_in_seconds = distance_to_sqr.sqrt() / 340.0;",
        "a 100-block trial spawner would be 0.29 s late instead of 2.5 s - the "
        "wrong number that was written in two places in this repo",
    ),
    (
        "delay: the gate is >= rather than >",
        "gate",
        LIB,
        "    if distance_to_sqr <= 100.0 {",
        "    if distance_to_sqr < 100.0 {",
        "exactly ten blocks would be delayed; vanilla's test is strict",
    ),
    (
        "delay: the row's flag is ignored and everything is delayed",
        "gate",
        LIB,
        "    let ticks = if row.distance_delay {",
        "    let ticks = if true {",
        "every distant block sound in the game would arrive late",
    ),
    (
        "delay: simplified to one multiply",
        "gate",
        LIB,
        """    let delay_in_seconds = distance_to_sqr.sqrt() / 40.0;
    Some((delay_in_seconds * 20.0) as i32)""",
        "    Some((distance_to_sqr.sqrt() * 0.5) as i32)",
        "neither 40 nor 20 is a power of two, so both steps round",
    ),
    # ---- the explode tail (w13/w14) --------------------------------------
    (
        "explode: the particle read as a holder (id + 1)",
        "gate",
        POPT,
        """    let id = r.varint().ok()?;
    let name = types.name(id)?.to_string();""",
        """    let id = r.varint().ok()? - 1;
    let name = types.name(id)?.to_string();""",
        "`registry(...)` is a RAW id and `holder(...)` is id+1; they sit one "
        "field apart in this packet",
    ),
    (
        "explode: an unknown particle assumes zero option bytes",
        "gate",
        POPT,
        "    let name = types.name(id)?.to_string();",
        '    let name = types.name(id).unwrap_or("?").to_string();',
        "right for 103 of 125 types today and a silent desync the moment a "
        "version adds a 23rd option-bearing one",
    ),
    (
        "explode: ExplosionParticleInfo read as two fields, not three",
        "gate",
        MOTION,
        """        let _scaling = r.f32()?;
        let _speed = r.f32()?;""",
        "        let _scaling = r.f32()?;",
        "every list entry after the first would be misaligned",
    ),
    (
        "explode: the sound read as a raw registry id",
        "gate",
        MOTION,
        "    let sound = crate::sounds::SoundRef::read(&mut r)?;",
        "    let sound = crate::sounds::SoundRef::Registry(r.varint()?);",
        "every explosion would name the sound one id along, and an inline "
        "definition would be read as the list count",
    ),
    # ---- the explosion sound itself (w15) --------------------------------
    (
        "explosion sound: volume 1.0 rather than 4.0",
        "gate",
        MOTION,
        "        volume: 4.0,",
        "        volume: 1.0,",
        "quarter of the carrying distance - `getRange` is 16 * max(volume, 1)",
    ),
    (
        "explosion sound: a single draw, so the pitch is pinned at 0.7",
        "gate",
        MOTION,
        """    let a = rng.next_float();
    let b = rng.next_float();
    let pitch = (1.0f32 + (a - b) * 0.2f32) * 0.7f32;""",
        """    let a = rng.next_float();
    let _b = rng.next_float();
    let pitch = (1.0f32 + (a - a) * 0.2f32) * 0.7f32;""",
        "every explosion identical - and the band's CONTAINMENT check alone "
        "would still pass, which is why w15 measures the spread",
    ),
    (
        "explosion sound: the source is MASTER rather than BLOCKS",
        "gate",
        MOTION,
        "        source: crate::sounds::SoundSource::Blocks,",
        "        source: crate::sounds::SoundSource::Master,",
        "the Blocks slider would stop controlling it",
    ),
    (
        "explosion sound: at the block centre rather than the packet's centre",
        "gate",
        MOTION,
        """        x: center.x,
        y: center.y,
        z: center.z,""",
        """        x: center.x.floor() + 0.5,
        y: center.y.floor() + 0.5,
        z: center.z.floor() + 0.5,""",
        "the BlockPos overload's centring, which this path does not take",
    ),
    # ---- the seed (w15's distinctness + w16), added after review ----------
    #
    # THE REVIEW'S HEADLINE DEFECT. `let seed = 0;` scored `soundshot` 35/35
    # exit 0 and `rewo-net` 1187/1187 exit 0 on the first commit of this
    # milestone: the seed was the entire reason `biome_noise::next_long` was
    # added, and nothing anywhere asserted anything about it. Routed BOTH ways,
    # because the fix ships a gate witness and unit tests and gotcha 0d says a
    # battery has to ask each of them separately.
    (
        "explosion sound: the seed is a constant [gate]",
        "gate",
        MOTION,
        "    let seed = rng.next_long();",
        "    let seed = 0;",
        "every explosion in the game plays the same one of "
        "`entity.generic.explode`'s FOUR variants - the sort of wrong no gate "
        "can hear, and until w15/w16 no gate could see it either",
    ),
    (
        "explosion sound: the seed is a constant [unit]",
        "net",
        MOTION,
        "    let seed = rng.next_long();",
        "    let seed = 0;",
        "the same break, asked of the unit suite rather than the gate",
    ),
    (
        "explosion sound: the seed is drawn FIRST",
        "gate",
        MOTION,
        """    let a = rng.next_float();
    let b = rng.next_float();
    let pitch = (1.0f32 + (a - b) * 0.2f32) * 0.7f32;
    let seed = rng.next_long();""",
        """    let seed = rng.next_long();
    let a = rng.next_float();
    let b = rng.next_float();
    let pitch = (1.0f32 + (a - b) * 0.2f32) * 0.7f32;""",
        "the draw ORDER, which `explosion_sound`'s own doc claimed was "
        "observable in a seeded gate while nothing observed it; the JVM oracle "
        "prints this ordering's seed (2912740758204167767) precisely so the "
        "witness cannot be satisfied by it",
    ),
    (
        "explosion sound: the seed is drawn FIRST [unit]",
        "net",
        MOTION,
        """    let a = rng.next_float();
    let b = rng.next_float();
    let pitch = (1.0f32 + (a - b) * 0.2f32) * 0.7f32;
    let seed = rng.next_long();""",
        """    let seed = rng.next_long();
    let a = rng.next_float();
    let b = rng.next_float();
    let pitch = (1.0f32 + (a - b) * 0.2f32) * 0.7f32;""",
        "the same reordering, asked of the unit suite",
    ),
    (
        "next_long: the low word is zero-extended [explosion oracle]",
        "net",
        NOISE,
        """        let upper = self.next(32) as i64;
        let lower = self.next(32) as i64;
        (upper << 32).wrapping_add(lower)""",
        """        let upper = self.next(32) as i64;
        let lower = self.next(32) as u32 as i64;
        (upper << 32) | lower""",
        "the same primitive break the `world`-routed entry covers, asked here "
        "of the only PRODUCTION caller - the review's point was that grading a "
        "primitive is not grading its use",
    ),
    # ---- normalize()'s zero guard (w9b), added after review ---------------
    (
        "bearing: normalize()'s zero guard is deleted",
        "gate",
        LIB,
        """    let dir = if dist < f64::from(1.0E-5_f32) {
        [0.0, 0.0, 0.0]
    } else {
        [d[0] / dist, d[1] / dist, d[2] / dist]
    };""",
        "    let dir = [d[0] / dist, d[1] / dist, d[2] / dist];",
        "a camera standing INSIDE the block gets a NaN sound position - the "
        "branch `camera_bearing_position`'s doc called reachable and no fixture "
        "reached, so deleting it left soundshot 36/36 and rewo-net 1190 green",
    ),
    # ---- unit-routed --------------------------------------------------
    (
        "next_long: the low word is zero-extended",
        "world",
        NOISE,
        """        let upper = self.next(32) as i64;
        let lower = self.next(32) as i64;
        (upper << 32).wrapping_add(lower)""",
        """        let upper = self.next(32) as i64;
        let lower = self.next(32) as u32 as i64;
        (upper << 32) | lower""",
        "half of all explosion seeds would be wrong, so half of all explosions "
        "would pick a different variant",
    ),
    (
        "camera_eye: answers Some before the server has positioned us",
        "net",
        PLAY,
        """        if !spawned {
            return None;
        }""",
        "",
        "a wither spawn before the first teleport would be placed against the "
        "origin - Rewo's `camera.isInitialized()`",
    ),
    (
        "camera_eye: hands out the FEET rather than the eye",
        "net",
        PLAY,
        "        Some([player.x, player.eye_y(), player.z])",
        "        Some([player.x, player.y, player.z])",
        "every global event's bearing off by 1.62 blocks vertically",
    ),
    (
        "engine: a delayed sound is played immediately",
        "net",
        ENGINE,
        """                    if let SoundEvent::AtDelayed { ticks, .. } = ev {
                        self.engine.play_delayed_ramped(instance, ramp, *ticks);
                        self.stats.queued_delayed += 1;
                        continue;
                    }""",
        "",
        "the whole feature would be inert with every decode witness green",
    ),
]


def build(pkg):
    p = subprocess.run(
        ["cargo", "build", "-p", pkg],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=900,
    )
    return p.returncode == 0


def run_gate():
    """(survived, reason). Survived == the gate PASSED, by EXIT CODE."""
    if not build("rewo-app"):
        return False, "build failed"
    try:
        p = subprocess.run(
            [EXE, "soundshot", "--check"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=420,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    # The summary line must be present, or a panic reads as a pass (M85).
    summary = [ln for ln in p.stdout.splitlines() if "witnesses (" in ln]
    if not summary:
        return False, f"exit {p.returncode}; no summary line (panic?)"
    failed = [
        ln.split(": ")[0].split("FAIL  ")[-1]
        for ln in p.stdout.splitlines()
        if "FAIL" in ln
    ]
    return p.returncode == 0, f"exit {p.returncode}; {', '.join(failed) or 'no failures'}"


def run_tests(pkg):
    """(survived, reason). Survived == every test PASSED, by EXIT CODE."""
    flag = "--bins" if pkg == "rewo-app" else "--lib"
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", pkg, flag],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=900,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    out = p.stdout + p.stderr
    # A crate whose tests fail to COMPILE prints no result line at all and must
    # not be mistaken for a clean kill of the logic (M110's shape).
    if "test result:" not in out:
        return False, f"exit {p.returncode}; no `test result` line (build failure?)"
    failing = [ln.strip() for ln in out.splitlines() if ln.strip().endswith("FAILED")]
    return p.returncode == 0, f"exit {p.returncode}; {len(failing)} failing"


def evaluate(route):
    if route == "gate":
        return run_gate()
    pkg = {"net": "rewo-net", "world": "rewo-world", "app": "rewo-app"}[route]
    return run_tests(pkg)


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m161] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control")

    # BASELINE. A battery run against an already-red tree scores KILLED for
    # everything and looks like a triumph.
    ok, why = run_gate()
    if not ok:
        print(f"BASELINE FAILED before any mutation: {why}")
        return 2
    print(f"baseline soundshot green ({why})")

    results = []
    for name, route, path, find, repl, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl)
            )
            survived, reason = evaluate(route)
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} - mutation may be left on disk"
            )
            # Gate-routed: the restore does not rebuild by itself, so the next
            # mutation would grade this one's binary.
            if route == "gate":
                build("rewo-app")
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} [{route}] {name}  ({reason})")
        results.append((name, verdict, why))

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
