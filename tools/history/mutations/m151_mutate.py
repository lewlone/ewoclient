"""M151's mutation battery — the tab list.

    python tools/m151_mutate.py

Same rules as `m149cf_mutate.py`: verdicts from the TEST RESULT LINE rather
than the exit code, a NO-OP CONTROL that must SURVIVE, restore in a `finally`,
a per-run timeout so a hang is a KILL rather than an outage, and a byte
comparison at the end rather than `git diff --quiet` (which cannot tell a
leftover mutation from uncommitted work).

**Most entries are routed through `tablistshot` rather than through the unit
tests, on purpose.** The unit suite runs on every commit and is well exercised;
the gate is new, and a gate that has quietly stopped testing its subject is the
failure mode this project keeps finding (M45, M41, M89, M94). So the mutations
whose claim the gate names — the sort, the fade, the ping bucket, the score's
colour, the face reservation, the draw order — are graded by it.

**A gate-routed mutation leaves the mutant's BINARY on disk** (the restore does
not rebuild), so this rebuilds once at the end before reporting. Without that,
the next manual `tablistshot` run grades whatever the last mutant compiled to.

The first run of this battery found four real witness gaps and one wrong
anchor; the entries below are the second cut. Its findings are recorded in the
milestone's commit messages.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
P = os.path.join("crates", "rewo-net", "src", "play.rs")
S = os.path.join("crates", "rewo-net", "src", "spawn_info.rs")
V = os.path.join("crates", "rewo-app", "src", "tab_list_view.rs")
U = os.path.join("crates", "rewo-gpu", "src", "hud.rs")

NET = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]
APP = ["cargo", "test", "-q", "-p", "rewo-app", "--bins"]
GATE = ["cargo", "run", "-q", "-p", "rewo-app", "--", "tablistshot", "--check"]

LISTED_OLD = (
    "        if let Some(listed) = e.listed {\n"
    "            if listed {\n"
    "                self.listed.insert(e.uuid);\n"
    "            } else {\n"
    "                self.listed.remove(&e.uuid);\n"
    "            }\n"
    "        }"
)
LISTED_NEW = (
    "        if let Some(listed) = e.listed {\n"
    "            if listed {\n"
    "                self.listed.insert(e.uuid);\n"
    "            }\n"
    "        }"
)

# (file, name, old, new, expected, command)
MUTATIONS = [
    (
        P,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// One entry of a `player_info_update` body.",
        "/// A single entry from the player-info packet.",
        "SURVIVED",
        NET,
    ),
    # --- the wire, and the state it lands in (M151a / M151e) ---------------
    (
        P,
        "UPDATE_LISTED only ever ADDS, so an unlisted player is never removed",
        LISTED_OLD,
        LISTED_NEW,
        "KILLED",
        NET,
    ),
    (
        P,
        "the listed flag is decoded but not kept, so nothing can filter on it",
        "    if has(3) {\n        e.listed = Some(r.bool()?);\n    }",
        "    if has(3) {\n        let _ = r.bool()?;\n    }",
        "KILLED",
        NET,
    ),
    (
        P,
        "a NULL display name is treated as unchanged rather than as a clear",
        "        e.display_name = Some(if r.bool()? { Some(r.nbt()?) } else { None });",
        "        e.display_name = if r.bool()? { Some(Some(r.nbt()?)) } else { None };",
        "KILLED",
        NET,
    ),
    (
        P,
        "the display name's payload is not walked, so the next action desyncs",
        "        e.display_name = Some(if r.bool()? { Some(r.nbt()?) } else { None });",
        "        let _ = r.bool()?;\n        e.display_name = Some(None);",
        "KILLED",
        NET,
    ),
    (
        P,
        "showHat defaults to FALSE where PlayerInfo's field initialises to true",
        "    pub fn show_hat(&self, uuid: u128) -> bool {\n        self.show_hats.get(&uuid).copied().unwrap_or(true)",
        "    pub fn show_hat(&self, uuid: u128) -> bool {\n        self.show_hats.get(&uuid).copied().unwrap_or(false)",
        "KILLED",
        NET,
    ),
    (
        P,
        "forgetting a player leaves them listed forever",
        "    pub fn forget(&mut self, uuid: u128) {\n        self.listed.remove(&uuid);",
        "    pub fn forget(&mut self, uuid: u128) {",
        "KILLED",
        NET,
    ),
    (
        P,
        "player_info_remove never forgets, so the list only ever grows",
        "                        self.tab_players.forget(uuid);",
        "                        let _ = uuid;",
        "SURVIVED",  # NAMED: the call site is in PlaySession, which no test can build
        NET,
    ),
    (
        S,
        "the login tail is read enforcesSecureChat-first, so onlineMode is the wrong byte",
        "    Ok(LoginTail {\n        online_mode: r.bool()?,\n        enforces_secure_chat: r.bool()?,\n    })",
        "    let enforces_secure_chat = r.bool()?;\n    Ok(LoginTail {\n        online_mode: r.bool()?,\n        enforces_secure_chat,\n    })",
        "KILLED",
        NET,
    ),
    # --- the resolver (M151b) ---------------------------------------------
    (
        V,
        "the sort key is the DISPLAY name rather than the profile name",
        "        entries.push(TabEntry {\n            uuid,\n            name,",
        "        let name = (l.display_name_of)(uuid)\n            .map(|c| rewo_world::chat_style::plain_text(&parse_component(&c, BASE, l.lang)))\n            .unwrap_or(name);\n        entries.push(TabEntry {\n            uuid,\n            name,",
        "KILLED",
        GATE,
    ),
    (
        V,
        "a display override is team-formatted too, doubling every team prefix",
        "    match display {\n        Some(c) => parse_component(c, base, lang),",
        "    match display {\n        Some(c) => rewo_net::sidebar::format_name_for_team(team, c, base, lang),",
        "KILLED",
        APP,
    ),
    (
        V,
        "the spectator fade is applied to EVERY row",
        "        let alpha = if row.spectator { SPECTATOR_ALPHA } else { NAME_ALPHA };",
        "        let alpha = SPECTATOR_ALPHA;",
        "KILLED",
        GATE,
    ),
    (
        V,
        "a spectator is drawn at full opacity, i.e. not faded at all",
        "pub const SPECTATOR_ALPHA: f32 = 144.0 / 255.0;",
        "pub const SPECTATOR_ALPHA: f32 = 1.0;",
        "KILLED",
        GATE,
    ),
    (
        V,
        "decorateName drops the spectator's italic",
        "    let base = ChatStyle { italic: spectator, ..BASE };",
        "    let base = BASE;",
        "KILLED",
        APP,
    ),
    (
        V,
        "F1 no longer hides the list",
        "    key_down && !hud_hidden",
        "    key_down",
        "KILLED",
        APP,
    ),
    (
        V,
        "the rows share ONE background fill, the way the sidebar's body does",
        "    for e in &layout.entries {\n        out.push(fill(e.background, tab_list::DEFAULT_ROW_BACKGROUND));\n    }",
        "    if let Some(e) = layout.entries.first() {\n        out.push(fill(e.background, tab_list::DEFAULT_ROW_BACKGROUND));\n    }",
        "KILLED",
        GATE,
    ),
    (
        V,
        "the ping sprite index is off by one",
        "        PingIcon::Unknown => 0,\n        PingIcon::Ping1 => 1,",
        "        PingIcon::Unknown => 1,\n        PingIcon::Ping1 => 2,",
        "KILLED",
        GATE,
    ),
    (
        V,
        "an unformatted score takes the SIDEBAR default (red) instead of yellow",
        "                    rewo_net::sidebar::PLAYER_LIST_DEFAULT_RGB,",
        "                    rewo_net::sidebar::SIDEBAR_DEFAULT_RGB,",
        "KILLED",
        GATE,
    ),
    (
        V,
        "the score is left-aligned to its span instead of right-aligned",
        "                push(&row.score, right - row.score_width, slot.name.1, NAME_ALPHA);",
        "                push(&row.score, right, slot.name.1, NAME_ALPHA);",
        "KILLED",
        APP,
    ),
    (
        V,
        "a HEARTS objective reserves nothing, so every name moves",
        "        Some(_) if hearts => ScoreColumn::Hearts,",
        "        Some(_) if hearts => ScoreColumn::None,",
        "KILLED",
        APP,
    ),
    (
        V,
        "the header wraps at the whole window width, not fifty pixels in",
        "        screen_width - tab_list::SCREEN_MARGIN,",
        "        screen_width,",
        "KILLED",
        GATE,
    ),
    (
        V,
        "maxNameWidth is measured off the profile name, not the drawn one",
        "        max_name_width = max_name_width.max(line_width(&name, l.width_of));",
        "        max_name_width = max_name_width.max((l.width_of)(&e.name, BASE));",
        "KILLED",
        APP,
    ),
    (
        V,
        "showHead is ignored, so an online server reserves no face",
        "        show_head: l.online_mode,",
        "        show_head: false,",
        "KILLED",
        GATE,
    ),
    (
        V,
        "a listed uuid with no profile name becomes a row named by its uuid",
        "        let Some(name) = (l.name_of)(uuid) else {\n            continue;\n        };",
        '        let name = (l.name_of)(uuid).unwrap_or_else(|| format!("{uuid:032x}"));',
        "KILLED",
        APP,
    ),
    # --- the pass (M151b) --------------------------------------------------
    (
        U,
        "the ping icons are emitted BEFORE the fills, so each row covers its own",
        "        for b in icons {\n            let r = match b.icon {",
        "        for b in icons.iter().take(0) {\n            let r = match b.icon {",
        "KILLED",
        GATE,
    ),
]


def run_tests_for(cmd):
    """Returns "ok", "failed", or "build" — three outcomes, not two.

    Reading only the exit code cannot tell a failing test from a failing BUILD,
    and so reports the thing the battery was built to find. The gate has no
    `test result:` line, so it is graded on its exit code with the build proved
    separately by `cargo build` returning 0 first.
    """
    gate = cmd is GATE
    for attempt in range(2):
        try:
            if gate:
                b = subprocess.run(
                    ["cargo", "build", "-q", "-p", "rewo-app"],
                    cwd=ROOT,
                    capture_output=True,
                    timeout=900,
                )
                if b.returncode != 0:
                    if attempt == 0:
                        time.sleep(3)
                        continue
                    sys.stderr.write(
                        (b.stdout + b.stderr).decode("utf-8", "replace")[-2000:]
                    )
                    return "build"
            p = subprocess.run(cmd, cwd=ROOT, capture_output=True, timeout=900)
        except subprocess.TimeoutExpired:
            subprocess.run(["taskkill", "/F", "/IM", "rewo.exe"], capture_output=True)
            subprocess.run(["taskkill", "/F", "/IM", "rewo-*.exe"], capture_output=True)
            return "failed"
        out = (p.stdout + p.stderr).decode("utf-8", "replace")
        if gate:
            return "ok" if p.returncode == 0 else "failed"
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
    for cmd in (NET, APP, GATE):
        if run_tests_for(cmd) != "ok":
            sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for rel, name, old, new, want, cmd in MUTATIONS:
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        crlf = b"\r\n" in snapshot
        text = snapshot.decode("utf-8").replace("\r\n", "\n")
        n = text.count(old)
        if n != 1:
            print("%-72s ANCHOR MATCHED %d TIMES" % (name[:72], n))
            bad += 1
            continue
        try:
            mutated = text.replace(old, new)
            if crlf:
                mutated = mutated.replace("\n", "\r\n")
            io.open(path, "wb").write(mutated.encode("utf-8"))
            r = run_tests_for(cmd)
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-72s %-10s (want %-10s) %s"
            % (name[:72], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

    leftover = [
        p for p in paths if io.open(os.path.join(ROOT, p), "rb").read() != snapshots[p]
    ]
    # Rebuild from the restored sources: a gate-routed mutation leaves the
    # MUTANT's binary behind, and the next manual run would grade it.
    subprocess.run(["cargo", "build", "-q", "-p", "rewo-app"], cwd=ROOT)
    print("-----")
    print(
        "files restored: %s"
        % ("no -- MUTATION LEFT ON DISK: %s" % leftover if leftover else "yes")
    )
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
