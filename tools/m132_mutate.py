"""M132's mutation harness — the scoreboard sidebar.

Copied from `tools/m125_mutate.py`, whose rules are inherited verbatim:

  * Verdicts come from the check's EXIT CODE, never from a substring of its
    output. M109 grepped for witness names, saw `ok` on every line, and missed
    that the command was red — and then a whole battery run against an
    already-failing command read KILLED for every entry.
  * Every battery contains a NO-OP CONTROL that must SURVIVE. A battery whose
    control dies is measuring a broken instrument, not the code.
  * The file is restored in a `finally`, AND its mtime is bumped, because
    cargo keys its rebuild on mtime and a restore that preserved the older one
    silently grades the previous binary (M92's harness bug).
  * Batteries are three or four entries so a run finishes inside the 10-minute
    tool cap. A battery killed by the cap leaves its mutation ON DISK.

    python tools/m132_mutate.py <battery>
"""

import os
import subprocess
import sys
import time

ROOT = "."

# Set on the ENVIRONMENT, not as a `VAR=value cmd` prefix: `shell=True` on
# Windows runs cmd.exe, where that prefix is not syntax at all.
CHECK_ENV = {}


def run(cmd, timeout):
    env = dict(os.environ)
    env.update(CHECK_ENV)
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True,
                           timeout=timeout, env=env)
        return p.returncode, (p.stdout + p.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        # A mutation that makes the check hang is killed by it. That is the
        # only verdict available and it is the right one.
        return 124, "<timed out>"


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)
    # cargo keys its rebuild on mtime; a restore that preserved the older one
    # would leave the NEXT run grading a stale binary.
    now = time.time()
    os.utime(path, (now, now))


def battery(name, check, timeout, mutations):
    print(f"=== {name} ===")
    print(f"    check: {check}")
    code, _ = run(check, timeout)
    if code != 0:
        print(f"    ABORT: the check is already red (exit {code}). "
              f"Every verdict below would read KILLED.")
        return 1
    print("    baseline green")

    bad = 0
    for path, old, new, expect, why in mutations:
        full = os.path.join(ROOT, path)
        orig = open(full, "rb").read()
        n = orig.decode("utf-8").count(old)
        if n != 1:
            print(f"  ?? {why}\n     anchor matched {n} times, not 1 — SKIPPED")
            bad += 1
            continue
        try:
            write(full, orig.decode("utf-8").replace(old, new, 1).encode("utf-8"))
            code, _ = run(check, timeout)
        finally:
            write(full, orig)
        got = "KILLED" if code != 0 else "SURVIVED"
        ok = got == expect
        if not ok:
            bad += 1
        print(f"  {'ok ' if ok else 'BAD'} {got:9s} (want {expect:9s})  {why}")
    print(f"    {'ALL AS EXPECTED' if bad == 0 else str(bad) + ' UNEXPECTED'}")
    return bad


SB = "crates/rewo-net/src/sidebar.rs"
LIVE = "crates/rewo-app/src/live_cmd.rs"
UNIT = "cargo test -q -p rewo-net --lib sidebar"
# BACKSLASHES. `shell=True` on Windows is cmd.exe, which does not accept
# `target/release/rewo.exe` as a command — it answers "'target' is not
# recognized" and the harness reads the baseline red. Which it did.
SHOT = ("cargo build -q --release -p rewo-app && "
        + "target" + os.sep + "release" + os.sep + "rewo.exe sidebarshot --check")

BATTERIES = {
    # -- the objective choice ------------------------------------------------
    "a": (
        "select_objective",
        UNIT,
        900,
        [
            (SB, "pub fn select_objective<'a>(",
                 "// no-op control\npub fn select_objective<'a>(",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (SB, "    let team_objective = team_display_slot(scoreboard, local_scoreboard_name)\n        .and_then(|slot| scoreboard.display_objective(slot));",
                 "    let team_objective: Option<&Objective> = None;\n    let _ = team_display_slot(scoreboard, local_scoreboard_name);",
                 "KILLED", "the team-colour slot never overrides the plain sidebar"),
            (SB, "    match team_objective {\n        Some(o) => Some(o),\n        None => scoreboard.display_objective(DisplaySlot::Sidebar),\n    }",
                 "    match team_display_slot(scoreboard, local_scoreboard_name) {\n        Some(_) => team_objective,\n        None => scoreboard.display_objective(DisplaySlot::Sidebar),\n    }",
                 "KILLED", "an EMPTY team slot blanks the sidebar instead of falling back"),
            (SB, "    DisplaySlot::ALL.get(3 + color as usize).copied()",
                 "    DisplaySlot::ALL.get(2 + color as usize).copied()",
                 "KILLED", "the colour -> slot offset is 2, not 3"),
        ],
    ),
    # -- the rows ------------------------------------------------------------
    "b": (
        "the row set and its order",
        UNIT,
        900,
        [
            (SB, "pub fn is_hidden(owner: &str) -> bool {\n    owner.starts_with('#')",
                 "pub fn is_hidden(owner: &str) -> bool {\n    owner.contains('#')",
                 "KILLED", "`isHidden` is a `contains`, not a prefix test"),
            (SB, "        .filter(|(owner, _)| !is_hidden(owner))",
                 "        .filter(|(owner, _)| !is_hidden(owner) || true)",
                 "KILLED", "hidden holders are not filtered at all"),
            (SB, "    match b.1.cmp(&a.1) {\n        std::cmp::Ordering::Equal => java_compare_ignore_case(a.0, b.0),",
                 "    match b.1.cmp(&a.1) {\n        std::cmp::Ordering::Equal => java_compare_ignore_case(b.0, a.0),",
                 "KILLED", "`.reversed()` is read as binding to the whole chain"),
            (SB, "    rows.sort_by(|a, b| compare_scores((a.0, a.1.value), (b.0, b.1.value)));\n    rows.truncate(MAX_ENTRIES);",
                 "    rows.truncate(MAX_ENTRIES);\n    rows.sort_by(|a, b| compare_scores((a.0, a.1.value), (b.0, b.1.value)));",
                 "KILLED", "the 15-row limit is applied BEFORE the sort"),
        ],
    ),
    # -- names and number formats --------------------------------------------
    "c": (
        "formatNameForTeam and formatValue",
        UNIT,
        900,
        [
            (SB, "        Some(NumberFormat::Blank) => Vec::new(),",
                 "        Some(NumberFormat::Blank) => parse_component(&Nbt::String(digits), base, lang),",
                 "KILLED", "a Blank format renders the digits rather than nothing"),
            (SB, "            ChatStyle { color: rgb_of(SIDEBAR_DEFAULT_RGB), ..base },",
                 "            base,",
                 "KILLED", "the default score format is the base colour, not RED"),
            (SB, "    let mut out = parse_component(&params.player_prefix, root, lang);\n    out.extend(parse_component(name, root, lang));",
                 "    let mut out = parse_component(&params.player_prefix, ChatStyle { color: root.color, ..base }, lang);\n    out.extend(parse_component(name, ChatStyle::plain(root.color), lang));",
                 "SURVIVED", "EQUIVALENT: `root` already differs from `base` only in its colour, and `base` carries no flags"),
            (SB, "        let charged = name_width + if score_width > 0 { spacer_width + score_width } else { 0 };",
                 "        let charged = name_width + spacer_width + score_width;",
                 "KILLED", "the `\": \"` spacer is charged even for an empty score"),
        ],
    ),
    # -- the geometry, against the pixel gate --------------------------------
    "d": (
        "the layout, through sidebarshot",
        SHOT,
        900,
        [
            (SB, "    let bottom = gui_height / 2 + height / 3;",
                 "    let bottom = gui_height / 2 + height / 3; // no-op control",
                 "SURVIVED", "NO-OP CONTROL: a comment"),
            (SB, "    let bottom = gui_height / 2 + height / 3;",
                 "    let bottom = gui_height / 2 + height / 2;",
                 "KILLED", "the panel is centred (height / 2) rather than a third below"),
            (SB, "    let right = gui_width - RIGHT_MARGIN + RIGHT_OVERHANG;",
                 "    let right = left + width;",
                 "KILLED", "the `+ 2` overhang is dropped (right = left + width)"),
        ],
    ),
    "e": (
        "the bands and the text, through sidebarshot",
        SHOT,
        900,
        [
            (SB, "    let header_background = Rect::corners(\n        left - LEFT_OVERHANG,\n        header_y - LINE_HEIGHT - 1,",
                 "    let header_background = Rect::corners(\n        left - LEFT_OVERHANG,\n        header_y - LINE_HEIGHT,",
                 "KILLED", "the header band is eight tall, not nine"),
            (LIVE, "        fill(layout.header_background, HEADER_BACKGROUND),\n        fill(layout.body_background, BODY_BACKGROUND),",
                   "        fill(layout.header_background, BODY_BACKGROUND),\n        fill(layout.body_background, HEADER_BACKGROUND),",
                   "KILLED", "the two band alphas are swapped (0.3 header / 0.4 body)"),
            (LIVE, "                    shadow: rewo_net::sidebar::DROP_SHADOW,",
                   "                    shadow: true,",
                   "KILLED", "the sidebar's text drops a shadow (the 5-arg overload's default)"),
        ],
    ),
    "f": (
        "the alignment and the emitter, through sidebarshot",
        SHOT,
        900,
        [
            (SB, "                    Some((right - e.score_width, y))",
                 "                    Some((right - e.score_width - 2, y))",
                 "KILLED", "the score is aligned two pixels left (the `left + width` reading)"),
            (SB, "        left + width / 2 - sidebar.title_width / 2,",
                 "        left + width / 2 - sidebar.title_width,",
                 "KILLED", "the title is not centred over the panel's width"),
            (LIVE, "    vec![\n        fill(layout.header_background, HEADER_BACKGROUND),",
                   "    vec![\n        fill(layout.body_background, BODY_BACKGROUND),\n        fill(layout.header_background, HEADER_BACKGROUND),",
                   "KILLED", "a third fill is emitted (the per-row reading `PlayerTabOverlay` uses)"),
        ],
    ),
}

if __name__ == "__main__":
    which = sys.argv[1]
    name, check, timeout, muts = BATTERIES[which]
    sys.exit(1 if battery(name, check, timeout, muts) else 0)
