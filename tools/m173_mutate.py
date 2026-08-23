"""M173's mutation battery — the volume sliders + the options wiring.

    python tools/m173_mutate.py [lo] [hi]

Checkers per mutation (M158): `gate` = `rewo optionshot --check`, `net` = the
rewo-net options/refresh/music tests, `world` = the options_screen tests.
`reap()` before every build (linker 1104); a timeout is a KILL; restore
byte-verified; the no-op control must SURVIVE.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
SI = os.path.join(ROOT, "crates/rewo-net/src/sound_instance.rs")
SE = os.path.join(ROOT, "crates/rewo-net/src/sound_engine.rs")
OS_ = os.path.join(ROOT, "crates/rewo-world/src/options_screen.rs")
SC = os.path.join(ROOT, "crates/rewo-world/src/screen.rs")
OPT = os.path.join(ROOT, "crates/rewo-net/src/options.rs")
LIVE = os.path.join(ROOT, "crates/rewo-app/src/live_cmd.rs")

STRAYS = ("rewo.exe", "rewo_net-*.exe", "rewo_world-*.exe")

MUTATIONS = [
    ("control: no change", SI,
     "pub struct CategoryVolumes {", "pub struct CategoryVolumes {",
     ["gate", "net", "world"], "MUST SURVIVE"),

    # ---- the gain math ---------------------------------------------------
    ("master is squared", SI,
     "        if source == SoundSource::Master {\n            self.slider(SoundSource::Master)\n        } else {",
     "        if false {\n            self.slider(SoundSource::Master)\n        } else {",
     ["net"], "final_volume: MASTER returns its own slider, not its square"),
    ("the refresh skips the MASTER-matches-all filter", SE,
     "            if source == l.instance.source || source == SoundSource::Master {",
     "            if source == l.instance.source {",
     ["net"], "refreshCategoryVolume: MASTER matches every playing instance"),
    ("the refresh drops the resolved-volume fold", SE,
     "                updates.push((\n                    l.channel,\n                    calculate_volume(\n                        instance_volume(l.instance.volume, l.resolved_volume),\n                        l.instance.source,\n                        &self.options,\n                        self.category_gain(l.instance.source),\n                    ),\n                ));",
     "                updates.push((\n                    l.channel,\n                    calculate_volume(\n                        l.instance.volume,\n                        l.instance.source,\n                        &self.options,\n                        self.category_gain(l.instance.source),\n                    ),\n                ));",
     ["net"], "instance.getVolume() folds the entry volume INSIDE the getter"),
    ("update_category_volume loses its refresh", SE,
     "        self.gain_by_source[source.ordinal() as usize] =\n            crate::sound_instance::mth_clamp(gain, 0.0, 1.0);\n        self.refresh_category_volume(source, device);",
     "        self.gain_by_source[source.ordinal() as usize] =\n            crate::sound_instance::mth_clamp(gain, 0.0, 1.0);",
     ["net"], "vanilla's put ALSO refreshes — the music fade must reach the channel"),
    ("the slider writes gainBySource", SE,
     "        self.engine.options.set_slider(source, value);\n        self.engine.refresh_category_volume(source, device);",
     "        self.engine.update_category_volume(source, value, device);",
     ["net"], "the slider never touches gainBySource — that channel is the music fade's"),

    # ---- the labels + layout --------------------------------------------
    ("the percent label rounds", OS_,
     '        format!("{caption}: {}%", (value * 100.0) as i32)',
     '        format!("{caption}: {}%", (value * 100.0).round() as i32)',
     ["world"], "vanilla truncates: 0.699999 renders 69%"),
    ("near-zero labels OFF", OS_,
     '    if value == 0.0 {',
     '    if value <= 0.004 {',
     ["world"], "only EXACTLY 0.0 is OFF; 0.004 is 0%"),
    ("the sound-page pairs shift by one", OS_,
     "            let ordinal = 1 + (row as i32 - 1) * 2 + col as i32;",
     "            let ordinal = (row as i32 - 1) * 2 + col as i32;",
     ["world"], "row 1 col 0 is MUSIC (ordinal 1), not MASTER again"),

    # ---- the slider math -------------------------------------------------
    ("the mouse math drops the half-handle inset", SC,
     "    (((mx - (x as f64 + 4.0)) / (width as f64 - 8.0)) as f32).clamp(0.0, 1.0)",
     "    (((mx - x as f64) / (width as f64 - 8.0)) as f32).clamp(0.0, 1.0)",
     ["world", "gate"], "(mx - (x + 4)) / (width - 8) — the +4 centres the handle under the cursor"),
    ("the handle divides by the full width", SC,
     "    x + (value * (width - 8) as f32) as i32",
     "    x + (value * width as f32) as i32",
     ["world", "gate"], "x + (int)(value * (width - 8)) — full width overhangs at value 1"),
    ("the arrow step is a fixed 5 percent", SC,
     "            let step = 1.0 / (w.width - 8) as f32;",
     "            let step = 0.05;",
     ["world"], "one handle-pixel: 1/302 on the master, 1/142 on a category slider"),

    # ---- the render ------------------------------------------------------
    ("the handle never highlights", LIVE,
     "        let engaged = dragging == Some(w.id);\n        let highlighted = w.active && (w.is_hovered(mouse) || engaged);",
     "        let engaged = dragging == Some(w.id);\n        let highlighted = false && (w.is_hovered(mouse) || engaged);",
     ["gate"], "hovered-or-engaged picks slider_handle_highlighted"),
    ("the handle draws the track sheet", LIVE,
     "            sheet: Sheet::SliderSheet(if highlighted { 3 } else { 2 }),",
     "            sheet: Sheet::SliderSheet(if highlighted { 1 } else { 0 }),",
     ["gate"], "the handle is its own 8x20 sheet, not a squeezed track"),

    # ---- the file --------------------------------------------------------
    ("out-of-range volumes are clamped instead of rejected", OPT,
     "                        t => t.parse::<f32>().ok().filter(|v| (0.0..=1.0).contains(v)),",
     "                        t => t.parse::<f32>().ok().map(|v| v.clamp(0.0, 1.0)),",
     ["net"], "the codec REJECTS out-of-range (vanilla logs + keeps default); it does not clamp"),
    ("legacy bools are not accepted", OPT,
     '                        "true" => Some(1.0),\n                        "false" => Some(0.0),',
     '                        "__never" => Some(1.0),\n                        "__never2" => Some(0.0),',
     ["net"], "Codec.withAlternative(.., BOOL, ..) — an old file reads true as 1.0"),
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


def unit(crate, filt):
    reap()
    code, out = run(["cargo", "test", "-q", "-p", crate, "--lib", filt], 600)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "tests or build failed"
    if "test result:" not in out:
        return 1, "no test result line"
    # A checker that ran ZERO tests is green about nothing — the first run of
    # this battery passed "options_screen screen" as ONE filter (with the
    # space), matched no test name, and four real kills read as SURVIVED.
    if " 0 passed" in out:
        return 1, f"filter {filt!r} matched zero tests"
    return 0, "ok"


def world():
    for f in ["options_screen", "slider"]:
        code, r = unit("rewo-world", f)
        if code != 0:
            return code, f"{f}: {r}"
    return 0, "ok"


def net():
    # options + the refresh/music families in one filtered run each.
    for f in ["options", "refresh", "the_music_gain", "master", "slider_path"]:
        code, r = unit("rewo-net", f)
        if code != 0:
            return code, f"{f}: {r}"
    return 0, "ok"


CHECKERS = {
    "gate": lambda: (lambda c: (c[0], "gate"))(run([EXE, "optionshot", "--check"], 600)),
    "net": net,
    "world": lambda: world(),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in m[4]}) or ["net"]
    print(f"[m173] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control; checkers {used}")
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
            if "gate" in run_checkers and not build():
                survived, reason = False, "build failed"
            else:
                for c in run_checkers:
                    code, r = CHECKERS[c]()
                    if code is None:
                        survived, reason = False, f"{c} TIMEOUT"
                        break
                    if code != 0:
                        survived, reason = False, f"{c} exit {code} ({r})"
                        break
                    reason = (reason + " " + f"{c} ok").strip()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), f"RESTORE FAILED {path}"
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
