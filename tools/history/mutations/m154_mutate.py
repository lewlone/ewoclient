"""M154's mutation battery — the `MUSIC_VOLUME` attribute.

Run: python tools/m154_mutate.py

Same discipline as `m152_mutate.py`: a no-op control that must SURVIVE (a
battery run against a red tree scores a perfect 100% and means nothing), exit
codes rather than substrings, a per-mutation timeout so a hang is a KILL rather
than an outage that leaves the mutant on disk, and a restore verified by BYTES
because `git diff` cannot tell a leftover mutation from uncommitted work.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PARSE = "crates/rewo-net/src/biome_parse.rs"
WORLD = "crates/rewo-world/src/lib.rs"
ENGINE = "crates/rewo-net/src/sound_engine.rs"

MUTATIONS = [
    (
        "control: no change",
        PARSE,
        'let v = attributes?.get("minecraft:audio/music_volume")?;',
        'let v = attributes?.get("minecraft:audio/music_volume")?;',
        "rewo-net",
        "music_volume",
        "MUST SURVIVE — otherwise every KILLED below is vacuous",
    ),
    (
        "parse: absence becomes silence",
        PARSE,
        'let v = attributes?.get("minecraft:audio/music_volume")?;',
        'let v = match attributes.and_then(|a| a.get("minecraft:audio/music_volume")) {\n        Some(v) => v,\n        None => return Some(0.0),\n    };',
        "rewo-net",
        "music_volume",
        "music would be muted in every biome, everywhere",
    ),
    (
        "parse: the unit range is trusted",
        PARSE,
        "Some((f as f32).clamp(0.0, 1.0))",
        "Some(f as f32)",
        "rewo-net",
        "music_volume",
        "a datapack could invert the mix or drive it into the limiter",
    ),
    (
        "parse: a modifier form is applied as a replace",
        PARSE,
        "    if v.get(\"modifier\").is_some() {\n        return None;\n    }",
        "    if false {\n        return None;\n    }",
        "rewo-net",
        "music_volume",
        "EXPECTED SURVIVOR, proven equivalent: a modifier form is a Compound and "
        "the Float/Double match below rejects it anyway, so the guard is dead code "
        "for this float-typed attribute. It is load-bearing on the two "
        "compound-valued siblings. See the comment at the guard.",
    ),
    (
        "resolve: the biome cannot override the base",
        WORLD,
        """        ctx.registry
            .biomes
            .get(id as usize)
            .and_then(|b| b.music_volume)
            .unwrap_or(base)""",
        "        base",
        "rewo-world",
        "music_volume",
        "exactly the pre-M154 bug: the Pale Garden would play music",
    ),
    (
        "engine: the probe is ignored and 1.0 passed",
        ENGINE,
        """        let outcome = self
            .music
            .tick(world.music_volume(), situational, is_active, false);""",
        "        let outcome = self.music.tick(1.0, situational, is_active, false);",
        "rewo-net",
        "music_volume",
        "the parse and the resolve would both be right and reach nothing",
    ),
]


def run(crate, filt):
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", crate, "--lib", filt],
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
    # One expected, proven-equivalent survivor (the modifier guard).
    return 0 if killed >= total - 1 else 1


if __name__ == "__main__":
    sys.exit(main())
