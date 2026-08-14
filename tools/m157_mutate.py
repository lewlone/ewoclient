"""M157's mutation battery — the two real options and options.txt.

Run: python tools/m157_mutate.py

Same discipline as m152/m154/m155/m156: a no-op control that must SURVIVE, exit
codes rather than substrings, a per-mutation timeout, and a restore verified by
BYTES.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OPT = "crates/rewo-net/src/options.rs"
SCR = "crates/rewo-world/src/options_screen.rs"

MUTATIONS = [
    (
        "control: no change",
        OPT,
        'pub const KEY_HIDE_LIGHTNING: &str = "hideLightningFlashes";',
        'pub const KEY_HIDE_LIGHTNING: &str = "hideLightningFlashes";',
        "rewo-net",
        "options",
        "MUST SURVIVE — otherwise every KILLED below is vacuous",
    ),
    (
        "the serialized name is the translation key",
        OPT,
        '        MusicFrequency::Default => "DEFAULT",\n        MusicFrequency::Frequent => "FREQUENT",\n        MusicFrequency::Constant => "CONSTANT",',
        '        MusicFrequency::Default => "options.music_frequency.default",\n        MusicFrequency::Frequent => "options.music_frequency.frequent",\n        MusicFrequency::Constant => "options.music_frequency.constant",',
        "rewo-net",
        "options",
        "the file would be unreadable by vanilla and reset every launch",
    ),
    (
        "the name match is case insensitive",
        OPT,
        '        "DEFAULT" => Some(MusicFrequency::Default),',
        '        s if s.eq_ignore_ascii_case("DEFAULT") => Some(MusicFrequency::Default),',
        "rewo-net",
        "options",
        "a lowercase name would parse where vanilla rejects it",
    ),
    (
        "the split takes every colon rather than the first",
        OPT,
        "pub fn split_line(line: &str) -> Option<(&str, &str)> {\n    line.split_once(':')\n}",
        "pub fn split_line(line: &str) -> Option<(&str, &str)> {\n    let mut p = line.split(':');\n    match (p.next(), p.next(), p.next()) {\n        (Some(a), Some(b), None) => Some((a, b)),\n        _ => None,\n    }\n}",
        "rewo-net",
        "options",
        "any value containing a colon would be rejected",
    ),
    (
        "a bad line aborts the whole load",
        OPT,
        "        for line in text.lines() {\n            o.apply_line(line);\n        }",
        "        for line in text.lines() {\n            if !o.apply_line(line) {\n                return Options::default();\n            }\n        }",
        "rewo-net",
        "options",
        "one corrupt entry would cost every other setting",
    ),
    (
        "saving REWRITES the file instead of merging",
        OPT,
        "        let mut out: Vec<String> = Vec::new();",
        "        let mut out: Vec<String> = Vec::new();\n        if !existing.is_empty() {\n            return mine.join(\"\\n\") + \"\\n\";\n        }",
        "rewo-net",
        "options",
        "a shared vanilla install would lose keybinds, volumes and render distance",
    ),
    (
        "the cycle does not wrap",
        OPT,
        "        self.music_frequency = FREQUENCY_CYCLE[(i + 1) % FREQUENCY_CYCLE.len()];",
        "        self.music_frequency = FREQUENCY_CYCLE[(i + 1).min(FREQUENCY_CYCLE.len() - 1)];",
        "rewo-net",
        "options",
        "the last value could never be cycled off",
    ),
    (
        "set_frequency stores without re-rolling",
        "crates/rewo-net/src/music.rs",
        "        self.frequency = frequency;\n        self.next_song_delay = self.frequency.next_song_delay(situational, &mut self.random);",
        "        self.frequency = frequency;",
        "rewo-net",
        "options",
        "changing the option would not take effect until the next track",
    ),
    (
        "screen: the column pitch is derived from the band",
        SCR,
        "pub const COLUMN_PITCH: i32 = 160;",
        "pub const COLUMN_PITCH: i32 = BAND_WIDTH / 2;",
        "rewo-world",
        "options_screen",
        "every right-hand widget would sit five pixels out",
    ),
    (
        "screen: rows advance on the button height",
        SCR,
        "        let y = header_h + i as i32 * ROW_HEIGHT;",
        "        let y = header_h + i as i32 * BUTTON_HEIGHT;",
        "rewo-world",
        "options_screen",
        "the five-pixel gap between rows would vanish",
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
    return 0 if killed == total else 1


if __name__ == "__main__":
    sys.exit(main())
