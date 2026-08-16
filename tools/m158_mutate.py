"""M158's mutation battery — the tab list's hearts and faces, in pixels.

Run: python tools/m158_mutate.py

GATE-routed, unlike m155's, which ran entirely through `cargo test` and so never
asked whether `tablistshot` could see the two emitters at all. It could not:
`tools/m158_gap.py` shows both bodies replaceable by `return Vec::new()` with
the gate green at 34/34.

Discipline per AGENT_LOOP_BRIEF and REWO_PLAN §0.0:

  * a no-op control that must SURVIVE, or every KILLED below is vacuous;
  * exit codes plus the gate's own summary line, never a substring of the body
    (M85: a panic must not read as a pass);
  * a per-mutation timeout, so a hang is a KILL rather than an outage whose
    `finally` never runs and leaves the mutant on disk;
  * a REBUILD after every restore — a gate-routed battery otherwise grades the
    previous mutant's BINARY against a clean tree;
  * the restore verified by BYTES, not by `git diff`, which cannot tell a
    leftover mutation from uncommitted work.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TL = os.path.join(ROOT, "crates/rewo-gpu/src/tab_list.rs")
VIEW = os.path.join(ROOT, "crates/rewo-app/src/tab_list_view.rs")
HUD = os.path.join(ROOT, "crates/rewo-gpu/src/hud.rs")
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")

MUTATIONS = [
    (
        "control: no change",
        TL,
        "pub const HEART_SPRITE_SIZE: i32 = 9;",
        "pub const HEART_SPRITE_SIZE: i32 = 9;",
        "MUST SURVIVE — otherwise every verdict below is vacuous",
    ),
    # ---- the gap M158 exists to close -----------------------------------
    (
        "hearts(): the emitter produces nothing",
        VIEW,
        """    gui_tick: i64,
) -> Vec<rewo_gpu::hud::HudBlit> {
    let mut out = Vec::new();""",
        """    gui_tick: i64,
) -> Vec<rewo_gpu::hud::HudBlit> {
    if true { return Vec::new(); }
    let mut out = Vec::new();""",
        "the health column would be blank — SURVIVED this gate before M158",
    ),
    (
        "faces(): the emitter produces nothing",
        VIEW,
        """    loaded_of: &dyn Fn(u128) -> bool,
) -> Vec<rewo_gpu::hud::HudBlit> {
    let mut out = Vec::new();""",
        """    loaded_of: &dyn Fn(u128) -> bool,
) -> Vec<rewo_gpu::hud::HudBlit> {
    if true { return Vec::new(); }
    let mut out = Vec::new();""",
        "every face would be missing — SURVIVED this gate before M158",
    ),
    # ---- the emitter's own geometry, which no unit test covers -----------
    (
        "hearts(): the blit x ignores the heart's own offset",
        VIEW,
        "                x: (left + b.dx) as f32,",
        "                x: left as f32,",
        "every heart in a row would stack at the column's left edge",
    ),
    (
        "hearts(): the sprite is drawn at the row's own size rather than 9x9",
        VIEW,
        "                w: tab_list::HEART_SPRITE_SIZE as f32,",
        "                w: (tab_list::HEART_SPRITE_SIZE - 2) as f32,",
        "the hearts would be squeezed and stop overlapping their neighbours",
    ),
    (
        "hearts(): a spectator's row draws hearts too",
        VIEW,
        "        let (Some((left, right)), Some(score)) = (slot.score_span, row.score_value)",
        "        let (Some((left, right)), Some(score)) = (slot.score_span.or(Some((0, 90))), row.score_value)",
        "a spectator would carry a health bar vanilla gives them none of",
    ),
    (
        "hearts(): the blink map is rebuilt rather than carried",
        VIEW,
        """        let health = states
            .entry(uuid)
            .or_insert_with(|| tab_list::HealthState::new(score));
        health.update(score, gui_tick);""",
        """        states.insert(uuid, tab_list::HealthState::new(score));
        let health = states.get_mut(&uuid).expect("just inserted");
        health.update(score, gui_tick);""",
        "nothing would ever blink — the failure mode `hearts`'s own doc names",
    ),
    # ---- the two-layer composite ----------------------------------------
    (
        "layering: a filled heart draws no container",
        TL,
        """        out.push(HeartBlit { dx: heart * per, sprite: container });
        if blink {""",
        "        if blink {",
        "the heart outline would vanish and the interior grey with it",
    ),
    (
        "absorption: the eleventh heart is an ordinary red one",
        TL,
        """                sprite: if heart >= 10 {
                    HeartSprite::AbsorbingFull
                } else {
                    HeartSprite::Full
                },""",
        "                sprite: HeartSprite::Full,",
        "gold absorption hearts would vanish",
    ),
    # ---- the face atlas geometry ----------------------------------------
    (
        "faces: the blit lands a pixel right of the layout's own rect",
        VIEW,
        "            x: rect.x as f32,",
        "            x: rect.x as f32 + 1.0,",
        "every face would sit a pixel right of the rect the LAYOUT placed",
    ),
    (
        "faces: the hat replaces the head instead of layering over it",
        VIEW,
        """        out.push(blit(false));
        // `if (hat) extractHat(..)` — a second blit over the first, never a
        // different sprite.
        if show_hat_of(e.uuid) {
            out.push(blit(true));
        }""",
        """        if show_hat_of(e.uuid) {
            out.push(blit(true));
        } else {
            out.push(blit(false));
        }""",
        "a hatted player would draw one blit, not two — the count f2 pins",
    ),
    (
        "faces: the flip inverts the DESTINATION v rather than the source",
        HUD,
        """                    let (v0, v1) = if flip {
                        // The SOURCE v inverts and the destination does not.
                        (
                            (y + 8) as f32 / ATLAS_H as f32,
                            y as f32 / ATLAS_H as f32,
                        )
                    } else {
                        (y as f32 / ATLAS_H as f32, (y + 8) as f32 / ATLAS_H as f32)
                    };""",
        """                    let (v0, v1) =
                        (y as f32 / ATLAS_H as f32, (y + 8) as f32 / ATLAS_H as f32);""",
        "a Dinnerbone would be the right way up — the exact trap §0.0 records",
    ),
    (
        "faces: the flip fires on the name alone, without the player loaded",
        VIEW,
        "        let flip = loaded_of(e.uuid) && is_upside_down_name(&e.name);",
        "        let flip = is_upside_down_name(&e.name);",
        "a Dinnerbone across the map would be upside down; vanilla needs the "
        "player in `level.getPlayerByUUID`",
    ),
]


def build():
    p = subprocess.run(
        ["cargo", "build", "-p", "rewo-app"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=900,
    )
    return p.returncode == 0


def gate():
    """(survived, reason). Survived == the gate PASSED."""
    try:
        p = subprocess.run(
            [EXE, "tablistshot", "--check"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=420,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    if "[tablistshot] PASS" not in p.stdout:
        failed = [
            ln.split(": ")[0].replace("[tablistshot] FAIL  ", "")
            for ln in p.stdout.splitlines()
            if "FAIL" in ln
        ]
        return False, f"exit {p.returncode}; {', '.join(failed) or 'no summary line'}"
    return p.returncode == 0, f"exit {p.returncode}"


def main():
    # Sliceable, because a gate-routed battery costs a rebuild per mutation and
    # a run killed by the 10-minute tool cap leaves its mutant ON DISK (its
    # `finally` never runs). The control is prepended to every slice, so no
    # slice can report a verdict without its own validity check.
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m158] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control")

    results = []
    for name, path, find, repl, why in selected:
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
            if not build():
                survived, reason = False, "build failed"
            else:
                survived, reason = gate()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
            build()  # gate-routed: the restore does not rebuild by itself
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
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
