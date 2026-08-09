"""M130's mutation harness — copied in shape from `tools/m125_mutate.py`.

Verdicts come from the EXIT CODE of the check, never from a substring of its
output (M109's finding, and M125's when the whole battery ran against a command
that never started). Every battery carries a NO-OP CONTROL expected to SURVIVE,
so a battery whose control dies is measuring a broken instrument.

Batteries `b`..`d` cost a release rebuild per mutation and must be run in the
background: M104's battery was killed by the 10-minute tool cap and left its
mutation ON DISK, because the `finally` never ran. If a run is interrupted,
`git status` before anything else.

    python tools/m130_mutate.py <battery>
"""

import os
import subprocess
import sys

ROOT = "."

LIVE = "crates/rewo-app/src/live_cmd.rs"
SCREEN = "crates/rewo-world/src/screen.rs"

# Built with `os.path.join`, never as a literal: cmd.exe cannot run
# `./target/...` at all, and a hand-written `target\release\rewo.exe` puts a
# carriage return in a Python string.
EXE = os.path.join("target", "release", "rewo.exe")
BUILD = "cargo build -q --release -p rewo-app"


def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True,
                           timeout=timeout)
        return p.returncode, (p.stdout + p.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return 124, "<timed out>"


def battery(name, check, timeout, mutations):
    out = []

    def say(s):
        out.append(s)
        sys.stdout.buffer.write((s + "\n").encode("utf-8", "replace"))
        sys.stdout.flush()

    say("=== %s ===" % name)
    say("    check: %s" % check)
    code, _ = run(check, timeout)
    if code != 0:
        say("    ABORT: the check is already red (exit %d). Every verdict "
            "below would read KILLED." % code)
        return 1
    say("    baseline green")

    bad = 0
    for path, old, new, expect, why in mutations:
        full = os.path.join(ROOT, path)
        orig = open(full, "rb").read()
        n = orig.decode("utf-8").count(old)
        if n != 1:
            say("  ?? %s\n     anchor matched %d times, not 1 - SKIPPED" % (why, n))
            bad += 1
            continue
        try:
            open(full, "wb").write(
                orig.decode("utf-8").replace(old, new, 1).encode("utf-8"))
            code, _ = run(check, timeout)
        finally:
            open(full, "wb").write(orig)
        got = "KILLED" if code != 0 else "SURVIVED"
        ok = got == expect
        if not ok:
            bad += 1
        say("  %s %-9s (want %-9s)  %s" % ("ok " if ok else "BAD", got, expect, why))
    say("    %s" % ("ALL AS EXPECTED" if bad == 0 else "%d UNEXPECTED" % bad))
    return bad


# The style-flag anchors are four-line blocks rather than the bare
# `style: text_style_of(span),`, because that line is in BOTH the title run and
# the death-screen run and an ambiguous anchor is skipped rather than guessed.
TITLE_STYLE = """                    alpha,
                    shadow: true,
                    style: text_style_of(span),"""
DEATH_STYLE = """                    alpha: 1.0,
                    shadow: true,
                    style: text_style_of(span),"""

BATTERIES = {
    "a": (
        "the model half: the draft's italic, its caret, and the two greys",
        "cargo test -q -p rewo-app --bins && cargo test -q -p rewo-world --lib",
        900,
        [
            (LIVE, "    let value = screen.input.value();",
                   "    let value = screen.input.value(); // no-op control",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (LIVE, """            shadow: true,
            style,
            text: value.clone(),""",
                   """            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: value.clone(),""",
                   "KILLED", "the draft loses its italic (the pre-M130 render)"),
            (LIVE, "            color_linear: field_color,\n            alpha: 1.0,\n            shadow: true,\n            style: rewo_gpu::text::TextStyle::PLAIN,\n            text: \"_\".to_string(),",
                   "            color_linear: color,\n            alpha: 1.0,\n            shadow: true,\n            style: rewo_gpu::text::TextStyle::PLAIN,\n            text: \"_\".to_string(),",
                   "KILLED", "the caret follows the draft's grey"),
            (LIVE, "            srgb_bytes_to_linear(0xAA_AAAA),",
                   "            rewo_net::chat_style::rgb_f32(0xAA_AAAA),",
                   "KILLED", "the draft grey is handed over as the /255 byte"),
            (LIVE, "                // byte `/255`, so it converts here.\n                color_linear: srgb_bytes_to_linear_f(color),",
                   "                // byte `/255`, so it converts here.\n                color_linear: color,",
                   "KILLED", "the search hint is handed over as the /255 byte"),
            (SCREEN, """        match self.kind {
            WidgetKind::Label { .. } | WidgetKind::MultiLabel { .. } => DEFAULT_LABEL,
            _ if self.active => DEFAULT_LABEL,
            _ => INACTIVE_LABEL,
        }""",
                     """        if self.active {
            DEFAULT_LABEL
        } else {
            INACTIVE_LABEL
        }""",
                     "KILLED", "a StringWidget greys although it is not a WithInactiveMessage"),
        ],
    ),
    "b": (
        "titleshot: the five flags, the bold measure, and the XP colour",
        "%s && %s titleshot --check" % (BUILD, EXE),
        900,
        [
            (LIVE, "    let mut pen = x;\n        for span in line {",
                   "    let mut pen = x;\n        // no-op control\n        for span in line {",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (LIVE, TITLE_STYLE,
                   """                    alpha,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle::PLAIN,""",
                   "KILLED", "m9b: the title drops all five Style flags"),
            (LIVE, "        .map(|s| rewo_gpu::text::width_styled(&s.text, advance, s.bold))",
                   "        .map(|s| rewo_gpu::text::width(&s.text, advance))",
                   "KILLED", "m9b: a bold title is measured style-blind and centres wrong"),
            (LIVE, "            color_linear: srgb_bytes_to_linear(color & 0x00FF_FFFF),",
                   "            color_linear: rewo_net::chat_style::rgb_f32(color & 0x00FF_FFFF),",
                   "KILLED", "m8: the XP level is handed over as the /255 byte"),
        ],
    ),
    "c": (
        "deathshot: the cause's flags, its bold measure, and two colours",
        "%s && %s deathshot --check" % (BUILD, EXE),
        900,
        [
            (LIVE, "        let mut pen = x;\n        for span in spans {",
                   "        let mut pen = x;\n        // no-op control\n        for span in spans {",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (LIVE, DEATH_STYLE,
                   """                    alpha: 1.0,
                    shadow: true,
                    style: rewo_gpu::text::TextStyle::PLAIN,""",
                   "KILLED", "m20: the death message drops all five Style flags"),
            (LIVE, "            let w = rewo_gpu::text::width_styled(&span.text, advance, span.bold);",
                   "            let w = rewo_gpu::text::width(&span.text, advance);",
                   "KILLED", "m21: a bold cause is penned out style-blind"),
            (LIVE, "                    color_linear: srgb_bytes_to_linear_f(span.color),\n                    alpha: 1.0,",
                   "                    color_linear: span.color,\n                    alpha: 1.0,",
                   "KILLED", "p10: the score's yellow is handed over as the /255 byte"),
        ],
    ),
    "d": (
        "deathshot p8 + serverlinkshot: the widget-label colour and its space",
        "%s && %s deathshot --check && %s serverlinkshot --check" % (BUILD, EXE, EXE),
        900,
        [
            (LIVE, "    let mut out = Vec::new();\n    // `color` arrives as vanilla's byte",
                   "    let mut out = Vec::new();\n    // no-op control\n    // `color` arrives as vanilla's byte",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (LIVE, "            color_linear: srgb_bytes_to_linear_f(color),\n            alpha: 1.0,\n            shadow: true,\n            style: rewo_gpu::text::TextStyle::PLAIN,\n            text: text.to_string(),",
                   "            color_linear: color,\n            alpha: 1.0,\n            shadow: true,\n            style: rewo_gpu::text::TextStyle::PLAIN,\n            text: text.to_string(),",
                   "KILLED", "p8: an inactive button label is handed over as the /255 byte"),
            (SCREEN, """        match self.kind {
            WidgetKind::Label { .. } | WidgetKind::MultiLabel { .. } => DEFAULT_LABEL,
            _ if self.active => DEFAULT_LABEL,
            _ => INACTIVE_LABEL,
        }""",
                     """        if self.active {
            DEFAULT_LABEL
        } else {
            INACTIVE_LABEL
        }""",
                     "KILLED", "serverlinkshot p10/p11: a dialog title greys"),
        ],
    ),
}

if __name__ == "__main__":
    which = sys.argv[1]
    name, check, timeout, muts = BATTERIES[which]
    sys.exit(1 if battery(name, check, timeout, muts) else 0)
