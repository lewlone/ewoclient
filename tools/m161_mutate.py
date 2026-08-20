"""M161's mutation battery — the wire-time flattens outside chat.

    python tools/m161_mutate.py            # every entry
    python tools/m161_mutate.py 0 1 2 3    # a slice, to stay inside a tool cap

Each entry names a source edit and the check that must go RED because of it.
Three rules this file exists to obey, all of them recorded in
`REWO_PLAN.md` §0.0:

* **A no-op CONTROL that must SURVIVE.** Without it a battery run against an
  already-broken tree reads KILLED for every entry and every kill is vacuous
  (M109 lost two whole batteries to exactly that).
* **A BASELINE at the top.** Every named check must be green before anything is
  mutated, for the same reason.
* **Route through what you claim coverage from** (gotcha 0d). M155 shipped a
  10/10 green battery that ran entirely through `cargo test` and never asked
  whether its gate could reach the emitters — it could not. So most entries here
  run a `*shot` gate, and a gate-routed entry needs a REBUILD after the restore,
  because putting the source back does not rebuild the binary and the next
  mutation would grade the previous mutant.

Exit codes only, never a substring: `blockentityshot`, `labelshot` and
`inventoryshot` are all fail-closed on a declared witness count and print `ok`
on every individual line while being red.

**The gap the first twelve entries had, and it is the one gotcha 0a names.**
Not one of them mutated a production CALL SITE - every entry edited a function
a gate reaches directly. An adversarial review then found three mutations that
survived everything, and all three were call-site edits: `styled_hover_name`'s
precedence (no fixture carried both names), `screen_tooltip` dropping the call
(no witness drove the real builder), and the app's two `collect_sign_text`
sites passing no table (`sg1`/`sg2` hand the collector a table of their own).
Entries 12-16 are the review's, and the sign one is not among them: it is a
compile error now, because `collect_session_sign_text` has no `lang` parameter
to pass the wrong thing to. What grades the one line left is `r49`, hand-run
against a real server - see the note at the bottom of this file.
"""

import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
BUILD_TIMEOUT = 420
RUN_TIMEOUT = 300

# (label, file, find, replace, check)
#
# `check` is either ("gate", <subcommand>) or ("test", <crate>, <filter>).
MUTATIONS = [
    (
        "CONTROL (no-op comment; MUST SURVIVE)",
        "crates/rewo-world/src/chat_style.rs",
        "pub fn flatten(tag: &Nbt, lang: Option<&Language>) -> String {",
        "pub fn flatten(tag: &Nbt, lang: Option<&Language>) -> String {\n    // m161 control",
        ("gate", "labelshot"),
    ),
    (
        "the nametag decode ignores the language table",
        "crates/rewo-net/src/metadata.rs",
        "Some(Some(rewo_world::chat_style::flatten(&nbt, kinds.lang)));",
        "Some(Some(rewo_world::chat_style::flatten(&nbt, None)));",
        ("gate", "labelshot"),
    ),
    (
        "the Optional's false arm is a no-op again (the pre-M161 decode)",
        "crates/rewo-net/src/metadata.rs",
        "Ok(false) => meta.custom_name = Some(None),",
        "Ok(false) => {}",
        ("gate", "labelshot"),
    ),
    (
        "a present-but-empty name is dropped again",
        "crates/rewo-net/src/metadata.rs",
        """                            meta.custom_name =
                                Some(Some(rewo_world::chat_style::flatten(&nbt, kinds.lang)));""",
        """                            let s = rewo_world::chat_style::flatten(&nbt, kinds.lang);
                            if !s.is_empty() {
                                meta.custom_name = Some(Some(s));
                            }""",
        ("gate", "labelshot"),
    ),
    (
        "flatten() is Nbt::to_plain_text again (no style, no `with`)",
        "crates/rewo-world/src/chat_style.rs",
        "    plain_text(&parse_component(tag, ChatStyle::WHITE, lang))",
        "    let _ = lang;\n    tag.to_plain_text()",
        ("gate", "labelshot"),
    ),
    (
        "flatten() is Nbt::to_plain_text again — the differential tests",
        "crates/rewo-world/src/chat_style.rs",
        "    plain_text(&parse_component(tag, ChatStyle::WHITE, lang))",
        "    let _ = lang;\n    tag.to_plain_text()",
        ("test", "rewo-net", "flatteners"),
    ),
    (
        "the sign's lines flatten without the table",
        "crates/rewo-world/src/block_entities.rs",
        "*slot = crate::chat_style::flatten(m, lang);",
        "*slot = crate::chat_style::flatten(m, None);",
        ("gate", "blockentityshot"),
    ),
    (
        "the styled hover name is never italic",
        "crates/rewo-app/src/live_cmd.rs",
        "    vec![if custom.is_some() { span.italic() } else { span }]",
        "    vec![span]",
        ("gate", "inventoryshot"),
    ),
    (
        "the tooltip resolves the name without the table",
        "crates/rewo-app/src/live_cmd.rs",
        """            Some(tag) => rewo_world::chat_style::flatten(tag, Some(lang)),
            None => translated.to_string(),""",
        """            Some(tag) => rewo_world::chat_style::flatten(tag, None),
            None => translated.to_string(),""",
        ("gate", "inventoryshot"),
    ),
    (
        "the crafting gate reads the MERGED name again (the live bug)",
        "crates/rewo-world/src/inventory.rs",
        "let named = text.is_some_and(|t| t.custom_name.is_some());",
        "let named = text.is_some_and(|t| t.custom_name.is_some() || t.item_name.is_some());",
        ("gate", "inventoryshot"),
    ),
    (
        "the nested container slot's hover name ignores the table",
        "crates/rewo-app/src/live_cmd.rs",
        "&e.hover_name(translated, Some(lang)),",
        "&e.hover_name(translated, None),",
        ("gate", "inventoryshot"),
    ),
    (
        "lore resolves without the table",
        "crates/rewo-app/src/live_cmd.rs",
        "                rewo_world::chat_style::flatten(line, Some(lang)),",
        "                rewo_world::chat_style::flatten(line, None),",
        ("gate", "inventoryshot"),
    ),
    # -- The review's entries. Each is a mutation an adversarial reviewer
    # -- applied to the shipped branch and watched every gate stay green.
    (
        "getHoverName's precedence swaps: item_name beats custom_name",
        "crates/rewo-app/src/live_cmd.rs",
        "    let named = custom.or_else(|| text.and_then(|t| t.item_name.as_ref()));",
        "    let named = text.and_then(|t| t.item_name.as_ref()).or(custom);",
        ("gate", "inventoryshot"),
    ),
    (
        "the production tooltip bypasses styled_hover_name entirely",
        "crates/rewo-app/src/live_cmd.rs",
        """        lines.push(styled_hover_name(
            text,
            translated,
            rarity_color(stack_rarity(
                Some(item_name),
                text.and_then(|t| t.rarity),
                text.is_some_and(|t| t.is_enchanted),
            )),
            lang,
        ));""",
        """        lines.push(vec![rewo_gpu::tooltip::Span::new(
            translated.to_string(),
            rarity_color(stack_rarity(
                Some(item_name),
                text.and_then(|t| t.rarity),
                text.is_some_and(|t| t.is_enchanted),
            )),
        )]);""",
        ("gate", "inventoryshot"),
    ),
    (
        "chat_component_text stops delegating and drops the table",
        "crates/rewo-world/src/chat_translate.rs",
        "    crate::chat_style::flatten(tag, lang)",
        "    let _ = lang;\n    crate::chat_style::flatten(tag, None)",
        ("test", "rewo-net", "the_four_spellings"),
    ),
    (
        "hud_state::plain open-codes to_plain_text again",
        "crates/rewo-net/src/hud_state.rs",
        "    chat_style::flatten(component, None)",
        "    component.to_plain_text()",
        ("test", "rewo-net", "the_four_spellings"),
    ),
    (
        "renders_empty open-codes nbt_text again",
        "crates/rewo-net/src/tab_list_text.rs",
        "    chat_style::flatten(component, None).is_empty()",
        "    crate::component_wire::nbt_text(component).is_empty()",
        ("test", "rewo-net", "the_four_spellings"),
    ),
]


def run(cmd, timeout):
    try:
        r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, timeout=timeout)
        return r.returncode
    except subprocess.TimeoutExpired:
        # A hang is a KILL, not an outage — see §0.0's M138d hazard.
        return 124


def build():
    return run(["cargo", "build", "-p", "rewo-app"], BUILD_TIMEOUT)


def check(spec):
    if spec[0] == "gate":
        if build() != 0:
            return "BUILD-FAIL"
        return "RED" if run([EXE, spec[1], "--check"], RUN_TIMEOUT) != 0 else "GREEN"
    crate, filt = spec[1], spec[2]
    code = run(["cargo", "test", "-p", crate, "--lib", filt], BUILD_TIMEOUT)
    return "RED" if code != 0 else "GREEN"


def read(path):
    with open(os.path.join(ROOT, path), "rb") as f:
        return f.read().decode("utf-8")


def write(path, text):
    p = os.path.join(ROOT, path)
    with open(p, "wb") as f:
        f.write(text.encode("utf-8"))
    # `mv`/`cp` preserve mtime and cargo then skips the rebuild, so the next
    # run grades the previous mutant (§0.0 gotcha 0b). Touch it forward.
    now = time.time() + 1
    os.utime(p, (now, now))


def main():
    picked = [int(a) for a in sys.argv[1:]] or list(range(len(MUTATIONS)))

    print("=== BASELINE ===")
    for spec in {m[4] for i, m in enumerate(MUTATIONS) if i in picked}:
        got = check(spec)
        print(f"  {spec} -> {got}")
        if got != "GREEN":
            print("BASELINE NOT GREEN — every verdict below would be vacuous. Stopping.")
            return 2

    killed, survived = 0, 0
    for i in picked:
        label, path, find, repl, spec = MUTATIONS[i]
        before = read(path)
        n = before.count(find)
        if n != 1:
            print(f"[{i}] {label}: ANCHOR MATCHED {n} TIMES — skipped, not survived")
            continue
        write(path, before.replace(find, repl, 1))
        try:
            got = check(spec)
        finally:
            write(path, before)
        is_control = label.startswith("CONTROL")
        verdict = "SURVIVED" if got == "GREEN" else ("KILLED" if got == "RED" else got)
        ok = (verdict == "SURVIVED") if is_control else (verdict == "KILLED")
        print(f"[{i}] {verdict:10s} {'OK ' if ok else '!! '} {label}  ({spec[0]})")
        if verdict == "KILLED":
            killed += 1
        elif verdict == "SURVIVED":
            survived += 1

    # Put the tree back to a built state so the next command is not grading a
    # stale binary.
    build()
    print(f"\nkilled={killed} survived={survived}")
    return 0


# -- The two entries no serverless gate can carry ---------------------------
#
# Both need `python tools/render_check.py`, i.e. a real server, so they are run
# by hand and their measurements written down rather than left as a claim.
#
# 1. `play.rs`'s single production `MetaKinds` construction site, `lang: None`.
#    `labelshot` builds its own `MetaKinds`, so nothing serverless sees it.
#    Measured: r48 goes from `1991 of 3306 frames carried "Zombie", 0 carried
#    the key` to `0 carried "Zombie", 3107 carried "entity.minecraft.zombie"`.
#
# 2. `collect_session_sign_text`'s body, `session.lang.as_deref()` -> `None`,
#    the one line left after the wrapper removed the per-call-site choice.
#    Measured: r49 goes from `3790 of 7188 frames carried "Dirt", 0 carried the
#    truncated key` to `0 of 5819 carried "Dirt", 2779 carried the truncated
#    key`. Not merely a red witness: two thousand frames of a raw translation
#    key sawn off by the sign board, which is what the bug looked like in game.
#
# After either, grep for the marker before anything else - an interrupted
# battery leaves its mutation on disk (gotcha -1a).

if __name__ == "__main__":
    sys.exit(main())
