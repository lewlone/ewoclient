"""M168's mutation battery — the survival HUD.

    python tools/m168_mutate.py [lo] [hi]

Each mutation names the CHECKER it is claiming coverage from (M158's gotcha:
route a battery through whatever you are claiming coverage from). Two kinds:

* `gate` — `rewo gaugeshot --check`, graded by EXIT CODE. Gate-routed, so the
  tree is rebuilt BEFORE the check and AFTER the restore (m164's finding: the
  restore does not rebuild by itself, and the next mutation would grade the
  previous mutant's binary).
* `unit-gpu` / `unit-net` / `unit-data` — `cargo test -p <crate> --lib`,
  graded by exit code AND the presence of a `test result:` line (M141's
  linker-1104 finding: a stray test binary holding the link output fails the
  BUILD, which also exits non-zero and would read as a kill).

A mutation that times out is a KILL and reaps stray binaries first
(`soundshot_mutate.py`'s `reap`), so a hung mutant cannot take the battery
down with the mutation still on disk. The no-op control runs with EVERY
checker the slice uses and must SURVIVE, or the battery returns 2 and every
KILLED below it is vacuous.

Discipline, inherited: exit codes rather than substrings, a per-mutation
timeout, a restore verified by BYTES, and `lo`/`hi` slices so one invocation
stays inside a ten-minute tool cap.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
SH = os.path.join(ROOT, "crates/rewo-gpu/src/survival_hud.rs")
HUD = os.path.join(ROOT, "crates/rewo-gpu/src/hud.rs")
LIVE = os.path.join(ROOT, "crates/rewo-app/src/live_cmd.rs")
META = os.path.join(ROOT, "crates/rewo-net/src/metadata.rs")
FX = os.path.join(ROOT, "crates/rewo-net/src/effects.rs")
HS = os.path.join(ROOT, "crates/rewo-net/src/hud_state.rs")
LPD = os.path.join(ROOT, "crates/rewo-net/src/local_player_data.rs")
TABLE = os.path.join(ROOT, "crates/rewo-data/src/mob_effect_table.rs")

STRAYS = ("rewo.exe", "rewo_gpu-*.exe", "rewo_net-*.exe", "rewo_data-*.exe", "rewo-*.exe")

# (name, path, find, repl, checkers, why)
MUTATIONS = [
    ("control: no change", SH,
     "pub const MAX_AIR_SUPPLY: i32 = 300;", "pub const MAX_AIR_SUPPLY: i32 = 300;",
     ["gate", "unit-gpu", "unit-net", "unit-data"],
     "MUST SURVIVE — otherwise every verdict below is vacuous"),

    # ---- the layout -------------------------------------------------------
    ("health rounds instead of ceiling", SH,
     "    let current_health = ceil_f(inp.health);",
     "    let current_health = inp.health.round() as i32;",
     ["gate"], "M3's bug: 0.3 hp shows no heart where vanilla shows half"),
    ("the food row fills from the left", SH,
     "        let xo = x_right - i * 8 - 9;\n        out.push(blit(xo, yo, 9, 9, HudIcon::Food(empty)));",
     "        let xo = x_right - 9 * 8 - 9 + i * 8;\n        out.push(blit(xo, yo, 9, 9, HudIcon::Food(empty)));",
     ["gate"], "identical pixels at ten full cells; wrong at any partial food"),
    ("the rows never compress", SH,
     "    let health_row_height = (10 - (num_health_rows - 2)).max(3);",
     "    let health_row_height = 10;",
     ["gate"], "three rows at 10 px instead of 9"),
    ("armour zero draws ten empties", SH,
     "    if armor <= 0 {\n        return;\n    }\n    let y = y_line_base",
     "    if armor < 0 {\n        return;\n    }\n    let y = y_line_base",
     ["gate"], "`if (armor > 0)` gates the whole row"),
    ("the armour row is one pixel low", SH,
     "    let y = y_line_base - (num_health_rows - 1) * health_row_height - 10;",
     "    let y = y_line_base - (num_health_rows - 1) * health_row_height - 9;",
     ["gate"], "`- 10` is the literal"),
    ("absorption hearts take the player's type", SH,
     "                let k = if kind == HeartKind::Withered {\n                    kind\n                } else {\n                    HeartKind::Absorbing\n                };",
     "                let k = kind;",
     ["unit-gpu"], "WITHERED when withered, else ABSORBING — never POISONED"),
    ("the ghost draws without blink", SH,
     "        if blink && halves < old_health {",
     "        if halves < old_health {",
     ["unit-gpu"], "a full-health frame would carry ten extra fills"),
    ("the full-bubble slack is zero ticks", SH,
     "    let full = air_bubble_count(current, max, -2);",
     "    let full = air_bubble_count(current, max, 0);",
     ["gate"], "three ceils with three offsets; `-2` is the full count's"),
    ("the air line ignores the vehicle rows", SH,
     "    let row_offset = ceil_d(f64::from(vehicle_hearts) / 10.0) - 1;\n    y_line_air - row_offset * 10",
     "    let row_offset = ceil_d(f64::from(vehicle_hearts) / 10.0);\n    y_line_air - row_offset * 10",
     ["gate"], "`(rows - 1) * 10`, which is -10 with no vehicle"),
    ("vehicle hearts cap at forty", SH,
     "        Some(v) => ((v.max_health + 0.5) as i32 / 2).min(30),",
     "        Some(v) => ((v.max_health + 0.5) as i32 / 2).min(40),",
     ["gate"], "the cap is 30 — three rows"),
    ("vehicle hearts halve before the cast", SH,
     "        Some(v) => ((v.max_health + 0.5) as i32 / 2).min(30),",
     "        Some(v) => (((v.max_health + 0.5) / 2.0) as i32).min(30),",
     ["gate"], "29 -> 14 (cast first) vs 14 either way? no: 29.5 / 2 = 14.75 -> 14 too; 30 -> 15 both. 31: (31.5 as i32)/2 = 15; 31.5/2 = 15.75 -> 15. Hmm — see the why of t6: 29 is graded; this may be EQUIVALENT on integer maxima and is kept to find out"),
    ("the effect order is the natural one", SH,
     "    sorted.sort_by(|a, b| effect_compare(b, a));",
     "    sorted.sort_by(|a, b| effect_compare(a, b));",
     ["gate"], "`Ordering.natural().reverse()`"),
    ("the fade starts at 201 ticks", SH,
     "    if e.ambient || !e.ends_within(200) {",
     "    if e.ambient || !e.ends_within(201) {",
     ["unit-gpu"], "`endsWithin(200)`"),
    ("the jump bar loses its first pixel", SH,
     "    P0 + (scale * (delta - 1) as f32).floor() as i32 + i32::from(scale > 0.0)",
     "    P0 + (scale * (delta - 1) as f32).floor() as i32",
     ["gate"], "`lerpDiscrete`'s `+ (alpha > 0 ? 1 : 0)`"),
    ("creative draws the hearts", SH,
     "    if inp.can_hurt {\n        player_health(inp, gui_w, gui_h, &mut out);\n    }",
     "    player_health(inp, gui_w, gui_h, &mut out);",
     ["gate"], "`canHurtPlayer()` gates extractPlayerHealth"),

    # ---- the pass ---------------------------------------------------------
    ("the half armour icon is the full one", HUD,
     "                ArmorSprite::Half => self.armor[1],",
     "                ArmorSprite::Half => self.armor[0],",
     ["gate"], "a swapped atlas slot; the pixel witness reads the PNG"),
    ("the effect fade is not a tint", HUD,
     "            tinted_quad(b.x, b.y, b.w, b.h, &r, uw, [1.0, 1.0, 1.0, b.alpha]);\n        }\n\n        // The chat backdrops go LAST",
     "            tinted_quad(b.x, b.y, b.w, b.h, &r, uw, [1.0, 1.0, 1.0, 1.0]);\n        }\n\n        // The chat backdrops go LAST",
     ["gate"], "`ARGB.white(alpha)` is a vertex tint"),

    # ---- the derivation ---------------------------------------------------
    ("armour rounds instead of flooring", LIVE,
     '    let armor = player_attr("armor", 0.0).floor() as i32;',
     '    let armor = player_attr("armor", 0.0).round() as i32;',
     ["gate"], "`Mth.floor(getAttributeValue(ARMOR))`"),
    ("an unknown game mode is creative", LIVE,
     "        .map(rewo_net::play::GameMode::is_survival)\n        .unwrap_or(true);\n    let player_attr",
     "        .map(rewo_net::play::GameMode::is_survival)\n        .unwrap_or(false);\n    let player_attr",
     ["gate"], "survival is the assumption the rest of the HUD makes"),
    ("the vehicle's synced attributes are ignored", LIVE,
     '                rewo_world::attributes::resolve(v.attributes, v.type_name, "max_health", r)',
     '                rewo_world::attributes::resolve(None, v.type_name, "max_health", r)',
     ["gate"], "a synced MAX_HEALTH must beat the supplier's default"),
    ("neutral effects are beneficial", LIVE,
     "                beneficial: def.is_some_and(|d| d.category.is_beneficial()),\n                color: def.map_or(0, |d| d.color),\n            }\n        })\n        .collect()\n}",
     "                beneficial: def.is_some_and(|d| d.category != rewo_data::mob_effect_table::MobEffectCategory::Harmful),\n                color: def.map_or(0, |d| d.color),\n            }\n        })\n        .collect()\n}",
     ["gate"], "`isBeneficial()` is `== BENEFICIAL`; NEUTRAL shares the harmful row"),

    # ---- the wire ---------------------------------------------------------
    ("the air arm wants a different serializer", META,
     "            (1, 1) => meta.air_supply = r.varint().ok(),",
     "            (1, 2) => meta.air_supply = r.varlong().ok(),",
     ["gate"], "INT is serializer 1"),
    ("the ambient bit is the visible bit", FX,
     "            ambient: u.flags & 1 != 0,",
     "            ambient: u.flags & 2 != 0,",
     ["gate"], "FLAG_AMBIENT = 1"),
    ("the first sync arms the window", HS,
     "        if self.flash_on_set_health {\n            let dmg = old_health - new_health;",
     "        if true {\n            let dmg = old_health - new_health;",
     ["gate", "unit-net"], "`flashOnSetHealth` skips the join-time sync"),
    ("the blink is on the even thirds", HS,
     "        let blink = self.health_blink_time > tick && (self.health_blink_time - tick) / 3 % 2 == 1;",
     "        let blink = self.health_blink_time > tick && (self.health_blink_time - tick) / 3 % 2 == 0;",
     ["gate"], "`/ 3L % 2L == 1L`"),
    ("the hud inputs wait for the flags guard", LPD,
     "    if let Some(air) = meta.air_supply {\n        data.air_supply = air;\n    }",
     "    if let (Some(air), Some(_)) = (meta.air_supply, meta.flags) {\n        data.air_supply = air;\n    }",
     ["gate"], "a diving player's packet carries no index 0"),
    ("the table calls glowing beneficial", TABLE,
     '    MobEffectDef { name: "glowing", category: MobEffectCategory::Neutral, color: 9740385 },',
     '    MobEffectDef { name: "glowing", category: MobEffectCategory::Beneficial, color: 9740385 },',
     ["unit-data"], "the generated table is pinned by a direct grep of the decompile"),
]


def reap():
    for pat in STRAYS:
        subprocess.run(["taskkill", "/F", "/IM", pat], capture_output=True)


def run(cmd, timeout):
    try:
        p = subprocess.run(
            cmd, cwd=ROOT, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        reap()
        return None, ""
    return p.returncode, p.stdout + p.stderr


def build():
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
    "unit-gpu": lambda: unit("rewo-gpu"),
    "unit-net": lambda: unit("rewo-net"),
    "unit-data": lambda: unit("rewo-data"),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in m[4]}) or ["gate"]
    print(f"[m168] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control; checkers {used}")
    results = []
    for name, path, find, repl, checkers, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        survived = True
        reason = ""
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
            run_checkers = used if name.startswith("control") else checkers
            if not build():
                survived, reason = False, "build failed"
            else:
                for c in run_checkers:
                    code, r = CHECKERS[c]()
                    if code is None:
                        survived, reason = False, f"{c} TIMEOUT (counted as killed)"
                        break
                    if code != 0:
                        survived, reason = False, f"{c} exit {code} ({r})"
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
    build()  # gate-routed: the restore does not rebuild by itself

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
