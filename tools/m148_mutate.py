"""M148's mutation battery — the music work (M145, M146, M147).

    python tools/m148_mutate.py

Same rules as its predecessors, and **run it alone** — see `m141g_mutate.py`'s
note on the interrupted-battery hazard, and `m141_mutate.py`'s on reading the
`test result:` line rather than the exit code.

**Do not `git add` while this is running.** Each mutation restores the working
TREE from a byte snapshot, so `git status` comes back clean — but `git add`
writes a separate snapshot into the index at the moment it runs. M142 committed
a live mutant that way.

## Why this battery exists at all, three milestones late

M145, M146 and M147 shipped without one, and M147 is the reason that was a
mistake: **every music unit test was green while the feature was completely
broken.** `situational_music` returned `None` everywhere in the Overworld, and
3137 tests, 34 gates and 45 render-check witnesses all passed over it. The unit
tests could not see the composition, and the composition was where the bug was.

So this battery is asking a specific question rather than a general one: *which
of these witnesses would have noticed?* Anything that survives is a witness that
would not have.

## Routing

Most of the music work is graded by `rewo-net --lib`, the records by
`rewo-world --lib`, and **the bundled dimension transcription only by the
`dimensioncheck` gate** — `builtin_registry_body()` is not reachable from any
unit test, so a battery running only `cargo test` would report its mutations
SURVIVED and be wrong about it. That is M142's `assets::bake` hazard again, and
it is handled the same way: a per-file runner.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
W = os.path.join("crates", "rewo-world", "src", "music.rs")
N = os.path.join("crates", "rewo-net", "src", "music.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
B = os.path.join("crates", "rewo-net", "src", "biome_parse.rs")
D = os.path.join("crates", "rewo-net", "src", "dimension_parse.rs")

MUTATIONS = [
    (
        N,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `MusicManager.STARTING_DELAY` — five seconds before the first track.",
        "/// MusicManager.STARTING_DELAY - five seconds before the first track.",
        "SURVIVED",
    ),
    # --- the records (M145) ------------------------------------------------
    (
        W,
        "creative beats underwater",
        "        if is_underwater && self.underwater_music.is_some() {\n            return self.underwater_music.as_ref();\n        }\n        if is_creative && self.creative_music.is_some() {",
        "        if is_creative && self.creative_music.is_some() {\n            return self.creative_music.as_ref();\n        }\n        if is_underwater && self.underwater_music.is_some() {",
        "KILLED",
    ),
    (
        W,
        "the creative arm does not fall through when creative is absent",
        "        if is_creative && self.creative_music.is_some() {",
        "        if is_creative {",
        "KILLED",
    ),
    (
        W,
        "a biome MERGES into the dimension base instead of replacing it",
        "            Some(over) => over.clone(),",
        "            Some(_) => dimension_base.clone(),",
        "KILLED",
    ),
    (
        W,
        "game music replaces, like the menu's",
        "        Music::new(sound, 12_000, 24_000, false)",
        "        Music::new(sound, 12_000, 24_000, true)",
        "KILLED",
    ),
    (
        W,
        "the menu's window is the game's",
        '        Music::new("minecraft:music.menu", 20, 600, true)',
        '        Music::new("minecraft:music.menu", 12_000, 24_000, true)',
        "KILLED",
    ),
    # --- the state machine (M145) ------------------------------------------
    (
        N,
        "the delay is decremented AFTER the test, not before",
        "            self.next_song_delay -= 1;\n            if self.next_song_delay <= 0 {",
        "            if self.next_song_delay <= 0 {",
        "KILLED",
    ),
    (
        N,
        "Mth.nextInt draws even when min >= max",
        "    if min_inclusive >= max_inclusive {\n        return min_inclusive;\n    }",
        "    if min_inclusive > max_inclusive {\n        return min_inclusive;\n    }",
        "KILLED",
    ),
    (
        N,
        "the CONSTANT case is checked before the null case",
        "        let Some(music) = music else {\n            return self.max_frequency();\n        };\n        if self == MusicFrequency::Constant {",
        "        if self == MusicFrequency::Constant {\n            return 100;\n        }\n        let Some(music) = music else {\n            return self.max_frequency();\n        };\n        if false {",
        "KILLED",
    ),
    (
        N,
        "the maxDelay cap is dropped, so startPlaying's MAX_VALUE park never lifts",
        "        self.next_song_delay = self.next_song_delay.min(music.max_delay);",
        "",
        "KILLED",
    ),
    (
        N,
        "canReplace ignores the identifier, so a replacing track restarts itself",
        "        music.replace_current_music && music.sound != current",
        "        music.replace_current_music",
        "KILLED",
    ),
    (
        N,
        "the stop does not clear on the same tick (the caller applies it later)",
        "                out.stop_current = true;\n                active = false;",
        "                out.stop_current = true;",
        "KILLED",
    ),
    (
        N,
        "stopPlaying drops its trailing STARTING_DELAY",
        "            .saturating_add(STARTING_DELAY);",
        ";",
        "KILLED",
    ),
    (
        N,
        "silence ASSIGNS the starting delay instead of raising to it",
        "            self.next_song_delay = self.next_song_delay.max(STARTING_DELAY);",
        "            self.next_song_delay = STARTING_DELAY;",
        "KILLED",
    ),
    (
        N,
        "the loading screen suppresses the start but still decrements",
        "        if self.current.is_none() && !on_loading_screen {",
        "        if self.current.is_none() {",
        "KILLED",
    ),
    # --- the composition (M146) --------------------------------------------
    (
        P,
        "the dragon plays wherever a boss bar does",
        "    if in_the_end && boss_music {",
        "    if boss_music {",
        "KILLED",
    ),
    (
        P,
        "isCreative is instabuild alone",
        "        let is_creative = self.abilities.instabuild && self.abilities.mayfly;",
        "        let is_creative = self.abilities.instabuild;",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the dimension base is EMPTY again (M147's bug)",
        "        let base = self\n            .active_dimension_type\n            .as_ref()\n            .and_then(|d| d.background_music.clone())\n            .unwrap_or_else(BackgroundMusic::empty);",
        "        let base = BackgroundMusic::empty();",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the music tick is never pushed (must SURVIVE)",
        "        self.tick_music();",
        "",
        "SURVIVED",
    ),
    # --- the engine side (M146) --------------------------------------------
    (
        E,
        "the engine ignores the outcome's stop",
        "        if outcome.stop_current {",
        "        if false && outcome.stop_current {",
        "KILLED",
    ),
    (
        E,
        "the started instance is not remembered, so isActive is always false",
        "                    self.music_instance = Some(id);",
        "                    let _ = id;",
        "KILLED",
    ),
    (
        E,
        "a refused track leaves the manager believing it is playing",
        "                    self.music_instance = None;\n                    self.music.stop_playing(situational);",
        "                    self.music_instance = None;",
        "KILLED",
    ),
    # --- the parse (M147/M148) ---------------------------------------------
    (
        B,
        "replace_current_music defaults to true",
        '            .and_then(as_bool)\n            .unwrap_or(false),',
        '            .and_then(as_bool)\n            .unwrap_or(true),',
        "KILLED",
    ),
    (
        B,
        "a malformed track invents a window instead of yielding none",
        '        min_delay: v.get("min_delay").and_then(Nbt::as_i64)? as i32,',
        '        min_delay: v.get("min_delay").and_then(Nbt::as_i64).unwrap_or(0) as i32,',
        "KILLED",
    ),
    (
        B,
        "an explicitly empty record reads as absent",
        '    let v = attributes?.get("minecraft:audio/background_music")?;',
        '    let v = attributes?.get("minecraft:audio/background_music")?;\n    if !matches!(v, Nbt::Compound(ref c) if !c.is_empty()) {\n        return None;\n    }',
        "KILLED",
    ),
    # --- the bundled transcription: gate-only (M147) ------------------------
    (
        D,
        "the bundled overworld loses its music (gate-only: dimensioncheck)",
        # The key appears TWICE — overworld and the_end — so the anchor carries
        # the creative track, which only the Overworld has. An anchor that
        # matches twice is a mutation that never ran.
        '                        "minecraft:audio/background_music",\n'
        "                        c(vec![\n"
        "                            (\n"
        '                                "creative",',
        '                        "minecraft:audio/background_music_DISABLED",\n'
        "                        c(vec![\n"
        "                            (\n"
        '                                "creative",',
        "KILLED",
    ),
]

# `builtin_registry_body` is unreachable from `cargo test`, so its mutations are
# graded by the gate that reads it. M142's `assets::bake` hazard, same fix.
GATE_ONLY = {D}

SUITES = {
    W: [["cargo", "test", "-p", "rewo-world", "--lib"]],
    N: [["cargo", "test", "-p", "rewo-net", "--lib"]],
    P: [["cargo", "test", "-p", "rewo-net", "--lib"]],
    E: [["cargo", "test", "-p", "rewo-net", "--lib"]],
    B: [["cargo", "test", "-p", "rewo-net", "--lib"]],
}


def run_gate():
    """`dimensioncheck --check` — the only thing that grades the bundled body."""
    if (
        subprocess.run(["cargo", "build", "-p", "rewo-app"], cwd=ROOT, capture_output=True).returncode
        != 0
    ):
        return "build"
    exe = os.path.join(ROOT, "target", "debug", "rewo.exe")
    try:
        p = subprocess.run(
            [exe, "dimensioncheck", "--check"], cwd=ROOT, capture_output=True, timeout=300
        )
    except subprocess.TimeoutExpired:
        return "failed"
    return "ok" if p.returncode == 0 else "failed"


def run_tests(rel=None):
    if rel in GATE_ONLY:
        return run_gate()
    suites = SUITES[rel] if rel else [s for v in SUITES.values() for s in v]
    # Deduplicate: several files share one suite.
    seen, uniq = set(), []
    for s in suites:
        k = tuple(s)
        if k not in seen:
            seen.add(k)
            uniq.append(s)
    for attempt in range(2):
        outs, rcs = [], []
        for args in uniq:
            try:
                p = subprocess.run(args, cwd=ROOT, capture_output=True, timeout=420)
            except subprocess.TimeoutExpired:
                for exe in ("rewo_net-*.exe", "rewo_world-*.exe"):
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

    print("BASELINE (unmutated, every suite) ...", end=" ", flush=True)
    if run_tests() != "ok" or run_gate() != "ok":
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for rel, name, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        text = snapshot.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-66s ANCHOR MATCHED %d TIMES" % (name[:66], n))
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
            "%-66s %-10s (want %-9s) %s"
            % (name[:66], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

    # **Rebuild before leaving.** A gate-routed mutation builds `rewo-app` from
    # the mutated source and the restore puts the source back WITHOUT
    # rebuilding, so `target/debug/rewo.exe` is left as the last mutant's
    # binary — and the next gate run grades that mutant against a clean tree.
    # It presents as a gate failing with `git status` clean, which is the most
    # confusing possible shape. Found by exactly that.
    if any(p in GATE_ONLY for p in paths):
        print("rebuilding rewo-app (the last gate mutation left a mutant binary) ...")
        subprocess.run(["cargo", "build", "-p", "rewo-app"], cwd=ROOT, capture_output=True)

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
