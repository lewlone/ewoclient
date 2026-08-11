"""M142's mutation battery — the ambient sound handlers.

    python tools/m142_mutate.py

Same rules as its predecessors, and **run it alone** — see `m141g_mutate.py`'s
note on the interrupted-battery hazard, and `m141_mutate.py`'s on reading the
`test result:` line rather than the exit code (which cannot tell a failing test
from a failing build).

**And do not `git add` while this is running.** Each mutation restores the
WORKING TREE from a byte snapshot when it finishes, so `git status` is clean
afterwards — but `git add` writes a SEPARATE snapshot into the index at the
moment it runs, and that moment can fall inside a mutation's window. M142
committed its `resolve` mutant exactly that way; the tree was already correct
by the time anyone looked, so the leftover-mutation check (which reads the
tree) could not see it. Stage before starting a battery or after its summary,
never across it.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
A = os.path.join("crates", "rewo-net", "src", "ambient_handlers.rs")
B = os.path.join("crates", "rewo-net", "src", "biome_parse.rs")
W = os.path.join("crates", "rewo-world", "src", "ambient.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")
L = os.path.join("crates", "rewo-world", "src", "lib.rs")
K = os.path.join("crates", "rewo-data", "src", "assets.rs")

MUTATIONS = [
    (
        A,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `SoundEvents.AMBIENT_UNDERWATER_LOOP_ADDITIONS`.",
        "/// SoundEvents.AMBIENT_UNDERWATER_LOOP_ADDITIONS.",
        "SURVIVED",
    ),
    # --- the underwater handler -------------------------------------------
    # NOT a rewrite into three independent rolls: that shape does not compile
    # against the `if/else if` chain it replaces, so it reported BUILD-FAIL —
    # a mutation that never ran rather than one that survived. The runtime
    # claim underneath it is that the bands PARTITION a single draw, which
    # widening either band breaks (the rare one is mutated below).
    (
        A,
        "the plain band is widened to the constant's own 0.01",
        "        } else if rand < 0.01 {",
        "        } else if rand < 0.011 {",
        "KILLED",
    ),
    (
        A,
        "a dry tick still draws (the gate moved below the draw)",
        "        if !underwater {\n            return;\n        }\n        let rand = rng.next_float();",
        "        let rand = rng.next_float();\n        if !underwater {\n            return;\n        }",
        "KILLED",
    ),
    (
        A,
        "the rare band is widened to the constant's 0.001 (independent-roll rates)",
        "        } else if rand < 0.001 {",
        "        } else if rand < 0.0011 {",
        "KILLED",
    ),
    # --- the edges ---------------------------------------------------------
    (
        A,
        "the falling edge STOPS the loop instead of leaving it to fade",
        "    if was_underwater && !is_underwater {",
        "    if false && was_underwater && !is_underwater {",
        "KILLED",
    ),
    (
        A,
        "the spectator gate is dropped from the edges",
        "    if spectator {\n        return;\n    }",
        "    if false && spectator {\n        return;\n    }",
        "KILLED",
    ),
    (
        A,
        "the rising edge mints no loop (only the enter one-shot)",
        "        out.push(SoundEvent::Tickable(TickableSound::UnderwaterLoop {\n            player,\n        }));",
        "        let _ = player;",
        "KILLED",
    ),
    # --- the bubble column -------------------------------------------------
    (
        A,
        "`wasInBubbleColumn` latches INSIDE the guard rather than outside",
        "            self.was_inside = true;\n        } else {",
        "            if self.first_tick_done && !spectator {\n                self.was_inside = true;\n            }\n        } else {",
        "KILLED",
    ),
    (
        A,
        "`firstTick` is ignored, so spawning inside a column fires",
        "            if !self.was_inside && self.first_tick_done && !spectator {",
        "            if !self.was_inside && !spectator {",
        "KILLED",
    ),
    (
        A,
        "drag picks the wrong sound (the default state is drag=true)",
        "                let sound = if drag_down {\n                    BUBBLE_WHIRLPOOL_INSIDE",
        "                let sound = if !drag_down {\n                    BUBBLE_WHIRLPOOL_INSIDE",
        "KILLED",
    ),
    (
        A,
        "the bubble sound is AMBIENT rather than the player's source",
        "                    source: SoundSource::Players,",
        "                    source: SoundSource::Ambient,",
        "KILLED",
    ),
    (
        A,
        "`inflate(0, -0.4, 0)` is read as growing rather than shrinking",
        "            aabb[1] + BUBBLE_Y_INSET,",
        "            aabb[1] - BUBBLE_Y_INSET,",
        "KILLED",
    ),
    # --- the mood ----------------------------------------------------------
    (
        A,
        "the mood's `- 1` is dropped, losing the freeze point",
        "            self.moodiness -= (block_light - 1) as f32 / mood.tick_delay as f32;",
        "            self.moodiness -= block_light as f32 / mood.tick_delay as f32;",
        "KILLED",
    ),
    (
        A,
        "the branches combine as max(sky, block) instead of excluding",
        "        if sky_light > 0 {",
        "        if sky_light > block_light {",
        "KILLED",
    ),
    (
        A,
        "the mood sound is placed AT the sampled block, not beyond it",
        "            let out_dist = dist + mood.sound_position_offset;",
        "            let out_dist = dist;",
        "KILLED",
    ),
    (
        A,
        "firing reloads a counter instead of resetting to 0",
        "            self.moodiness = 0.0;",
        "            self.moodiness = 0.5;",
        "KILLED",
    ),
    (
        A,
        "the three offset draws collapse to one (a diagonal, not a cube)",
        "        let oy = rng.next_int(span) - mood.block_search_extent;\n        let oz = rng.next_int(span) - mood.block_search_extent;",
        "        let oy = ox;\n        let oz = ox;",
        "KILLED",
    ),
    # --- the additions -----------------------------------------------------
    (
        A,
        "only the FIRST addition is tried (an Option, not a list)",
        "    for add in &a.additions {",
        "    for add in a.additions.iter().take(1) {",
        "KILLED",
    ),
    (
        A,
        "the tick-chance compare is non-strict, so 0.0 sometimes fires",
        "        if rng.next_double() < add.tick_chance {",
        "        if rng.next_double() <= add.tick_chance {",
        "KILLED",
    ),
    (
        A,
        "an addition is given a position (it is relative at the origin)",
        "            out.push(SoundEvent::Instance(SoundInstance::for_ambient_addition(\n                add.sound.clone(),\n            )));",
        "            out.push(SoundEvent::Instance(SoundInstance::for_ambient_mood(\n                add.sound.clone(),\n                1.0,\n                2.0,\n                3.0,\n            )));",
        "KILLED",
    ),
    # --- the decode --------------------------------------------------------
    (
        B,
        "the modifier form decodes as an EMPTY record (silencing the base)",
        '    if !matches!(v, Nbt::Compound(_)) || v.get("modifier").is_some() {\n        return None;\n    }',
        "    if !matches!(v, Nbt::Compound(_)) {\n        return None;\n    }",
        "KILLED",
    ),
    (
        B,
        "`additions` accepts only the List form (the compactListCodec trap)",
        "        Nbt::List(items) => items.iter().filter_map(one).collect(),\n        other => one(other).into_iter().collect(),",
        "        Nbt::List(items) => items.iter().filter_map(one).collect(),\n        _ => Vec::new(),",
        "KILLED",
    ),
    (
        B,
        "the mood's offset is read from `sound_position_offset`, not `offset`",
        '        sound_position_offset: n.get("offset").and_then(as_f64)?,',
        '        sound_position_offset: n.get("sound_position_offset").and_then(as_f64)?,',
        "KILLED",
    ),
    (
        B,
        "a partial mood decodes with zeros instead of voiding the mood",
        '        tick_delay: n.get("tick_delay").and_then(as_i32)?,',
        '        tick_delay: n.get("tick_delay").and_then(as_i32).unwrap_or(0),',
        "KILLED",
    ),
    # --- the resolution ----------------------------------------------------
    (
        W,
        "the biome MERGES with the dimension base instead of replacing it",
        "        match biome.and_then(|b| b.ambient_sounds.as_ref()) {\n            Some(over) => over.clone(),",
        "        match biome.and_then(|b| b.ambient_sounds.as_ref()) {\n            Some(over) => AmbientSounds {\n                loop_sound: over.loop_sound.clone().or(dimension_base.loop_sound.clone()),\n                mood: over.mood.clone().or(dimension_base.mood.clone()),\n                additions: over.additions.clone(),\n            },",
        "KILLED",
    ),
    (
        W,
        "the quart conversion truncates toward zero instead of flooring",
        "    (c.floor() as i32) >> 2",
        "    (c as i32) >> 2",
        "KILLED",
    ),
    (
        W,
        "LEGACY_CAVE_SETTINGS grows a loop it does not have",
        "            loop_sound: None,\n            mood: Some(AmbientMood::legacy_cave()),",
        '            loop_sound: Some("minecraft:ambient.cave".into()),\n            mood: Some(AmbientMood::legacy_cave()),',
        "KILLED",
    ),
    # --- the instance ------------------------------------------------------
    (
        E,
        "the underwater loop is constructed at volume 0.0 (never plays)",
        "            let inst = SoundInstance {\n                looping: true,\n                delay: 0,\n                volume: 1.0,\n                relative: true,",
        "            let inst = SoundInstance {\n                looping: true,\n                delay: 0,\n                volume: 0.0,\n                relative: true,",
        "KILLED",
    ),
    (
        E,
        "the underwater loop is not head-locked (it swims around your head)",
        "                volume: 1.0,\n                relative: true,\n                ..SoundInstance::bare(\n                    crate::ambient_handlers::UNDERWATER_LOOP,",
        "                volume: 1.0,\n                relative: false,\n                ..SoundInstance::bare(\n                    crate::ambient_handlers::UNDERWATER_LOOP,",
        "KILLED",
    ),
    (
        E,
        "the sub-sound LOOPS (it is a one-shot)",
        "            let inst = SoundInstance {\n                looping: false,\n                delay: 0,\n                volume: 1.0,\n                relative: true,\n                ..SoundInstance::bare(sound, SoundSource::Ambient)",
        "            let inst = SoundInstance {\n                looping: true,\n                delay: 0,\n                volume: 1.0,\n                relative: true,\n                ..SoundInstance::bare(sound, SoundSource::Ambient)",
        "KILLED",
    ),
    # --- the bubble-column scan (M142c) ------------------------------------
    (
        L,
        "the scan reads through a missing chunk instead of emptying",
        "                if !self.columns.contains_key(&(cx, cz)) {\n                    return None;\n                }",
        "                let _ = (cx, cz);",
        "KILLED",
    ),
    (
        L,
        "the scan is Y-major, so a lower-Y block beats a lower-Z one",
        "        for z in z0..=z1 {\n            for y in y0..=y1 {",
        "        for y in y0..=y1 {\n            for z in z0..=z1 {",
        "KILLED",
    ),
    (
        L,
        "the scan is X-major, so a lower-X block beats a lower-Y one",
        "            for y in y0..=y1 {\n                for x in x0..=x1 {",
        "            for x in x0..=x1 {\n                for y in y0..=y1 {",
        "KILLED",
    ),
    (
        L,
        "the max bound is CEILED rather than floored",
        "            aabb[3].floor() as i32,\n            aabb[4].floor() as i32,\n            aabb[5].floor() as i32,",
        "            aabb[3].ceil() as i32,\n            aabb[4].ceil() as i32,\n            aabb[5].ceil() as i32,",
        "KILLED",
    ),
    # These two are graded by `blockentityshot`, NOT by `cargo test` — see
    # `run_gate`. A battery that ran only the unit tests would call both
    # SURVIVED and be wrong about it.
    (
        K,
        "the drag property is looked up by its JAVA field name",
        '                    .and_then(|p| p.get("drag"))',
        '                    .and_then(|p| p.get("drag_down"))',
        "KILLED",
    ),
    (
        K,
        "an unreadable drag falls back to the block default (a whirlpool)",
        '                    .map(|v| v == "true");',
        '                    .map(|v| v == "true")\n                    .or(Some(true));',
        "KILLED",
    ),
    # --- composition roots (PlaySession has no test module anywhere) --------
    (
        P,
        "COMPOSITION ROOT: the bubble handler is never ticked (must SURVIVE)",
        "        self.ambient_bubble.tick(found, spectator, pos, &mut out);",
        "        let _ = (found, spectator);",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the handlers are never ticked (must SURVIVE)",
        "        self.tick_ambient_sounds();",
        "",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the previous-tick snapshot never updates (must SURVIVE)",
        "        self.was_underwater = underwater;",
        "",
        "SURVIVED",
    ),
]


def run_gate():
    """`blockentityshot --check`, which is the ONLY thing that grades the bake.

    `assets::bake` needs the client jar and the datagen report, so its
    `bubble_column_drag` derivation is not reachable from any unit test — a
    battery running only `cargo test` would report every `assets.rs` mutation
    as SURVIVED and be wrong about it. That is M45's hazard from the other
    side: a harness that does not run the check cannot grade what it covers.
    """
    if subprocess.run(
        ["cargo", "build", "-p", "rewo-app"], cwd=ROOT, capture_output=True
    ).returncode != 0:
        return "build"
    exe = os.path.join(ROOT, "target", "debug", "rewo.exe")
    try:
        p = subprocess.run(
            [exe, "blockentityshot", "--check"],
            cwd=ROOT,
            capture_output=True,
            timeout=300,
        )
    except subprocess.TimeoutExpired:
        return "failed"
    return "ok" if p.returncode == 0 else "failed"


def run_tests(rel=None):
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note.

    `rel` names the mutated file, so the bake's witnesses can be reached: they
    live in a gate rather than in `cargo test`.
    """
    if rel == K:
        return run_gate()
    for attempt in range(2):
        outs, rcs = [], []
        for args in (
            ["cargo", "test", "-p", "rewo-world", "--lib"],
            ["cargo", "test", "-p", "rewo-net", "--lib"],
        ):
            try:
                p = subprocess.run(args, cwd=ROOT, capture_output=True, timeout=300)
            except subprocess.TimeoutExpired:
                for exe in ("rewo_world-*.exe", "rewo_net-*.exe"):
                    subprocess.run(["taskkill", "/F", "/IM", exe], capture_output=True)
                return "failed"
            outs.append((p.stdout + p.stderr).decode("utf-8", "replace"))
            rcs.append(p.returncode)
        joined = "\n".join(outs)
        if "test result: FAILED" in joined:
            return "failed"
        if all("test result: ok" in o for o in outs) and all(r == 0 for r in rcs):
            return "ok"
        if attempt == 0:
            time.sleep(3)
            continue
        sys.stderr.write(joined[-2000:] + "\n")
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
            print("%-62s ANCHOR MATCHED %d TIMES" % (name[:62], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            r = run_tests(rel)
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-62s %-10s (want %-9s) %s"
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
