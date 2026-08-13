"""M149c-f's mutation battery — the clock map and all three flash consumers.

    python tools/m149cf_mutate.py

Same rules as `m149_mutate.py` (which covers M149a's schedule): verdicts from
the TEST RESULT LINE rather than the exit code, a NO-OP CONTROL that must
SURVIVE, restore in a `finally`, a per-run timeout so a hang is a KILL, and a
byte comparison at the end rather than `git diff --quiet`.

Each mutated file's witnesses live in one crate, so each entry runs that
crate's suite rather than the workspace — three times faster, same signal.

Two entries expect SURVIVED and are recorded as such rather than left looking
untested; each says why in its own name.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
P = os.path.join("crates", "rewo-net", "src", "play.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
L = os.path.join("crates", "rewo-app", "src", "live_cmd.rs")

NET = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]
APP = ["cargo", "test", "-q", "-p", "rewo-app", "--bins"]
CRATES = {P: NET, E: NET, L: APP}

# (file, name, old, new, expected)
MUTATIONS = [
    (
        P,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `ClientClockManager.tick` — one delta, applied to every instance.",
        "/// One tick of every world clock.",
        "SURVIVED",
    ),
    # --- the clock map (M149c) --------------------------------------------
    (
        P,
        "total_ticks stops minting, so an unsent clock is 0 forever",
        "        self.clocks.push((\n            id,\n            WorldClock {",
        "        if true {\n            return 0;\n        }\n        self.clocks.push((\n            id,\n            WorldClock {",
        "KILLED",
    ),
    (
        P,
        "a minted clock gets rate 0.0 instead of vanilla's 1.0",
        "                rate: 1.0,\n                last_game_time: self.last_game_time,",
        "                rate: 0.0,\n                last_game_time: self.last_game_time,",
        "KILLED",
    ),
    (
        P,
        "a minted clock anchors at 0, not the manager's shared time",
        "                last_game_time: self.last_game_time,",
        "                last_game_time: 0,",
        "KILLED",
    ),
    (
        P,
        "handle_updates overwrites BEFORE it ticks",
        "        self.tick(game_time);\n        for &(holder, total, partial, rate) in entries {",
        "        for &(holder, total, partial, rate) in entries {",
        "KILLED",
    ),
    (
        P,
        "an absent default_clock falls back to the overworld's",
        "    let Some(name) = active.and_then(|d| d.default_clock.as_deref()) else {\n        return 0;\n    };",
        '    let name = active\n        .and_then(|d| d.default_clock.as_deref())\n        .unwrap_or("minecraft:overworld");',
        "KILLED",
    ),
    (
        P,
        "an unknown clock name resolves to id 0 anyway",
        "    let Some(id) = world_clock_ids.iter().position(|n| n == name) else {\n        return 0;\n    };",
        "    let id = world_clock_ids.iter().position(|n| n == name).unwrap_or(0);",
        "KILLED",
    ),
    (
        P,
        "the day-tick fallback mints instead of peeking",
        "    let day = overworld_id\n        .and_then(|id| clocks.peek(id))\n        .map(|c| c.total)\n        .unwrap_or(next);",
        "    let day = overworld_id\n        .map(|id| clocks.total_ticks(id))\n        .unwrap_or(next);",
        "KILLED",
    ),
    # --- the flash's level lifetime ---------------------------------------
    (
        P,
        "the flash survives a dimension change instead of being rebuilt",
        "        *self.end_flash = end_flash_for_dimension(&def);",
        "        // MUTANT: carried across the level change",
        "KILLED",
    ),
    (
        P,
        "every dimension gets a flash, not only an END skybox",
        "    def.skybox\n        .has_end_flashes()\n        .then(rewo_world::end_flash::EndFlashState::default)",
        "    Some(rewo_world::end_flash::EndFlashState::default())",
        "KILLED",
    ),
    (
        P,
        "NAMED SURVIVOR: the login call site (a composition root with no test seam — the RULE it calls is killed above)",
        "        self.end_flash = end_flash_for_dimension(&active.def);",
        "        self.end_flash = Some(rewo_world::end_flash::EndFlashState::default());",
        "SURVIVED",
    ),
    # --- the delayed sound (M149f) ----------------------------------------
    (
        E,
        "the queued tickable is NOT ticked before it plays",
        "            let ramp = match ramp {\n                Some(mut r) => {\n                    r.tick(&mut inst, world);\n                    Some(r)\n                }\n                None => None,\n            };",
        "            // MUTANT: no tick before play",
        "KILLED",
    ),
    (
        E,
        "the End flash plays immediately instead of being queued",
        "                        self.engine.play_delayed_ramped(\n                            instance,\n                            ramp,\n                            rewo_world::end_flash::SOUND_DELAY_IN_TICKS,\n                        );",
        "                        self.engine\n                            .play_ramped(instance, ramp, &self.sounds, world, device);",
        "KILLED",
    ),
    (
        E,
        "EXPECTED SURVIVOR: the directional instance attenuates (no witness reads the channel's attenuation call)",
        "                attenuation: Attenuation::None,\n                x: cx + dx * 10.0,",
        "                attenuation: Attenuation::Linear,\n                x: cx + dx * 10.0,",
        "SURVIVED",
    ),
    # --- the lightmap term (M149d) ----------------------------------------
    (
        L,
        "the flash MULTIPLIES the sky factor instead of adding",
        "        sky_factor: dim.sky_light_factor * sky.light_factor + end_flash.sky_factor_bonus(),",
        "        sky_factor: dim.sky_light_factor * sky.light_factor * (1.0 + end_flash.sky_factor_bonus()),",
        "KILLED",
    ),
    (
        L,
        "boss world fog SUPPRESSES the flash instead of thirding it",
        "        if self.boss_world_fog {\n            self.intensity / 3.0\n        } else {\n            self.intensity\n        }",
        "        if self.boss_world_fog {\n            0.0\n        } else {\n            self.intensity\n        }",
        "KILLED",
    ),
    (
        L,
        "EXPECTED SURVIVOR: the render half gated on hideLightningFlash too (the const is false, so both readings agree until an options screen exists)",
        "    let s = session.end_flash()?;\n    Some((s.intensity(partial), s.x_angle(), s.y_angle()))",
        "    let s = session.end_flash()?;\n    if HIDE_LIGHTNING_FLASH {\n        return None;\n    }\n    Some((s.intensity(partial), s.x_angle(), s.y_angle()))",
        "SURVIVED",
    ),
]


def run_tests_for(cmd):
    """Returns "ok", "failed", or "build" — three outcomes, not two.

    Reading only the exit code cannot tell a failing test from a failing
    BUILD, and so reports the thing the battery was built to find.
    """
    for attempt in range(2):
        try:
            p = subprocess.run(cmd, cwd=ROOT, capture_output=True, timeout=600)
        except subprocess.TimeoutExpired:
            subprocess.run(["taskkill", "/F", "/IM", "rewo_net-*.exe"], capture_output=True)
            subprocess.run(["taskkill", "/F", "/IM", "rewo-*.exe"], capture_output=True)
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
    for cmd in (NET, APP):
        if run_tests_for(cmd) != "ok":
            sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for rel, name, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        crlf = b"\r\n" in snapshot
        text = snapshot.decode("utf-8").replace("\r\n", "\n")
        n = text.count(old)
        if n != 1:
            print("%-66s ANCHOR MATCHED %d TIMES" % (name[:66], n))
            bad += 1
            continue
        try:
            mutated = text.replace(old, new)
            if crlf:
                mutated = mutated.replace("\n", "\r\n")
            io.open(path, "wb").write(mutated.encode("utf-8"))
            r = run_tests_for(CRATES[rel])
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-66s %-10s (want %-10s) %s"
            % (name[:66], verdict, want, "ok" if ok else "<<< UNEXPECTED")
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
