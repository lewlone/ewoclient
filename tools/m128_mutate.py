"""M128's mutation harness — clickable chat text.

Copied from `tools/m125_mutate.py`, with its two rules kept:

* verdicts come from the check's EXIT CODE, never from a substring of its
  output (M109), and
* every battery opens with a BASELINE run and contains a NO-OP CONTROL that
  must SURVIVE — a battery whose control dies is measuring a broken
  instrument, not the code (M125's `shell=True` trap).

The restore is in a `finally` AND bumps the file's mtime, because `cp`/`mv`
preserve the *older* timestamp and cargo then skips the rebuild and silently
grades the mutated binary (M92's 0b).

    python tools/m128_mutate.py <battery> [<battery> ...]
    python tools/m128_mutate.py all
"""

import os
import subprocess
import sys
import time

ROOT = "."


def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True,
                           timeout=timeout)
        return p.returncode, (p.stdout + p.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return 124, "<timed out>"


def battery(name, check, timeout, mutations):
    print(f"=== {name} ===")
    print(f"    check: {check}")
    code, out = run(check, timeout)
    if code != 0:
        print(f"    ABORT: the check is already red (exit {code}). "
              f"Every verdict below would read KILLED.")
        print(out[-1500:])
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
            open(full, "wb").write(
                orig.decode("utf-8").replace(old, new, 1).encode("utf-8"))
            os.utime(full, (time.time(), time.time()))
            code, _ = run(check, timeout)
        finally:
            open(full, "wb").write(orig)
            os.utime(full, (time.time(), time.time()))
        got = "KILLED" if code != 0 else "SURVIVED"
        ok = got == expect
        if not ok:
            bad += 1
        print(f"  {'ok ' if ok else 'BAD'} {got:9s} (want {expect:9s})  {why}")
    print(f"    {'ALL AS EXPECTED' if bad == 0 else str(bad) + ' UNEXPECTED'}")
    return bad


EVENTS = "crates/rewo-world/src/chat_events.rs"
STYLE = "crates/rewo-world/src/chat_style.rs"
SPLIT = "crates/rewo-world/src/string_splitter.rs"
ACTIVE = "crates/rewo-world/src/active_text.rs"
SCREEN = "crates/rewo-world/src/chat_screen.rs"
CHAT = "crates/rewo-world/src/chat.rs"
URI = "crates/rewo-app/src/uri_open.rs"

W = "cargo test -q -p rewo-world --lib"

BATTERIES = {
    "a": (
        "the event decode and its two refusals",
        f"{W} chat_events",
        600,
        [
            (EVENTS, "pub fn parse_events(tag: &Nbt) -> Option<ChatEvents> {",
                     "pub fn parse_events(tag: &Nbt) -> Option<ChatEvents> { // no-op control",
                     "SURVIVED", "NO-OP CONTROL: a comment"),
            (EVENTS, '        "open_file" => return None,',
                     '        "open_file" => ClickEvent::CopyToClipboard(String::new()),',
                     "KILLED", "open_file decodes to something (allowFromServer ignored)"),
            (EVENTS, '    if protocol != "http" && protocol != "https" {\n        return None;\n    }',
                     "    if false {\n        return None;\n    }",
                     "KILLED", "any scheme is an allowed link protocol"),
            (EVENTS, "    let scheme = uri_scheme(uri)?;",
                     "    let scheme = uri_scheme(uri).unwrap_or(\"http\");",
                     "KILLED", "a missing scheme is not the `scheme == null` throw"),
            (EVENTS, "    if !chars.next()?.is_ascii_alphabetic() {",
                     "    if false {",
                     "KILLED", "a scheme need not start with a letter"),
            (EVENTS, "    if uri.is_empty() || !uri.bytes().all(|b| (0x21..=0x7E).contains(&b)) {",
                     "    if uri.is_empty() {",
                     "KILLED", "a space or control character is accepted in a URI"),
            (EVENTS, "        .all(|c| c != '\\u{00A7}' && c >= ' ' && c != '\\u{007F}')",
                     "        .all(|c| c >= ' ' && c != '\\u{007F}')",
                     "KILLED", "CHAT_STRING drops the section-sign clause"),
            (EVENTS, "            if page <= 0 {\n                return None;\n            }",
                     "            let page = page.max(1);",
                     "KILLED", "POSITIVE_INT clamps instead of refusing"),
            (EVENTS, '        Some(0) => ("minecraft", &s[1..]),',
                     '        Some(0) => ("", &s[1..]),',
                     "KILLED", "a leading colon means an EMPTY namespace"),
            (EVENTS, "            click: child.click.clone().or_else(|| parent.click.clone()),",
                     "            click: child.click.clone(),",
                     "KILLED", "applyTo replaces the group instead of merging per field"),
            (EVENTS, '        "show_text" => HoverEvent::ShowText(tag.get("value")?.clone()),',
                     '        "show_text" => HoverEvent::ShowText(tag.clone()),',
                     "KILLED", "show_text carries the wrapper instead of its `value`"),
        ],
    ),
    "b": (
        "the style's three new fields",
        f"{W} chat_style",
        600,
        [
            (STYLE, "    pub fn span(&self, text: impl Into<String>) -> ChatSpan {",
                    "    pub fn span(&self, text: impl Into<String>) -> ChatSpan { // no-op control",
                    "SURVIVED", "NO-OP CONTROL: a comment"),
            (STYLE, "        lower => ChatStyle::colored(\n            rgb_f32(NAMED_COLORS[legacy_color_index(lower)?].1),\n            style.events.clone(),\n        ),",
                    "        lower => ChatStyle::colored(\n            rgb_f32(NAMED_COLORS[legacy_color_index(lower)?].1),\n            None,\n        ),",
                    "KILLED", "a legacy colour code clears the click event"),
            (STYLE, "    style.events = ChatEvents::apply_to(chat_events::parse_events(tag).as_ref(), parent.events.as_ref());",
                    "    style.events = chat_events::parse_events(tag).map(std::sync::Arc::new);",
                    "KILLED", "a child does not inherit its parent's events"),
            (STYLE, "            events: self.events.clone(),\n        }\n    }\n\n    /// `Style.getClickEvent()`.",
                    "            events: None,\n        }\n    }\n\n    /// `Style.getClickEvent()`.",
                    "KILLED", "the span does not carry the style's events"),
        ],
    ),
    "c": (
        "the events survive the wrap",
        f"{W} string_splitter",
        600,
        [
            (SPLIT, "pub fn find_styled_line_break(",
                    "// no-op control\npub fn find_styled_line_break(",
                    "SURVIVED", "NO-OP CONTROL: a comment"),
            # The M128 design claim: events on the SPAN alone are dropped by
            # `splitAt`'s tail rebuild. Emulated by making `style()` forget
            # them, which is exactly the pre-M128 shape of the data.
            (STYLE, "            obfuscated: self.obfuscated,\n            events: self.events.clone(),\n        }\n    }\n\n    /// `Style.getClickEvent()` for this run.",
                    "            obfuscated: self.obfuscated,\n            events: None,\n        }\n    }\n\n    /// `Style.getClickEvent()` for this run.",
                    "KILLED", "the split style forgets the events (events on the span alone)"),
        ],
    ),
    "d": (
        "the active-area box model",
        f"{W} active_text",
        600,
        [
            (ACTIVE, "pub fn prepare(",
                     "// no-op control\npub fn prepare(",
                     "SURVIVED", "NO-OP CONTROL: a comment"),
            (ACTIVE, "                    // The one override: the ADVANCE, not the sprite's right.\n                    right: pen + advance,",
                     "                    right: pen + GLYPH_LEFT + FONT_HEIGHT,",
                     "KILLED", "activeRight is the sprite cell rather than the advance"),
            (ACTIVE, "    glyphs.extend(empties);",
                     "    let mut all = empties;\n    all.extend(glyphs);\n    let glyphs = all;",
                     "KILLED", "empty areas are visited BEFORE the glyphs"),
            (ACTIVE, "                    span: index,\n                    empty: true,\n                });",
                     "                    span: index,\n                    empty: true,\n                });\n                bounds = Some(match bounds {\n                    None => (pen, y, pen + advance, y + EMPTY_HEIGHT),\n                    Some((l, t, r, b)) => (l.min(pen), t.min(y), r.max(pen + advance), b.max(y + EMPTY_HEIGHT)),\n                });",
                     "KILLED", "an empty area contributes to bounds"),
            (ACTIVE, "        x >= self.left && x < self.right && y >= self.top && y < self.bottom",
                     "        x >= self.left && x <= self.right && y >= self.top && y < self.bottom",
                     "KILLED", "the right edge is inclusive, so the seam double-counts"),
            (ACTIVE, "                let shear_left = if span.italic {\n                    SHEAR_TOP.min(SHEAR_BOTTOM)",
                     "                let shear_left = if span.italic {\n                    SHEAR_TOP.max(SHEAR_BOTTOM)",
                     "KILLED", "the italic left shear takes max instead of min"),
            (ACTIVE, "        if worth_reporting {\n            self.result = Some(style);\n        }",
                     "        self.result = worth_reporting.then_some(style);",
                     "KILLED", "an unclickable style CLEARS an earlier find"),
            (ACTIVE, "style.click().is_some() || (self.include_insertions && style.insertion().is_some());",
                     "style.click().is_some() || (style.insertion().is_some());",
                     "KILLED", "an insertion is reported without shift"),
            (ACTIVE, "    prepared.areas.iter().filter(|a| a.contains(x, y)).next_back()",
                     "    prepared.areas.iter().find(|a| a.contains(x, y))",
                     "KILLED", "the FIRST match wins rather than the last"),
            (ACTIVE, "    let (l, t, r, b) = prepared.bounds?;",
                     "    let (l, t, r, b) = prepared.bounds.unwrap_or((f32::MIN, f32::MIN, f32::MAX, f32::MAX));",
                     "KILLED", "a null bounds does not early-out"),
            (ACTIVE, "    c == ' ' || c == '\\u{200C}'",
                     "    false",
                     "KILLED", "nothing is an empty glyph"),
        ],
    ),
    "e": (
        "the click resolution",
        f"{W} chat_screen",
        600,
        [
            (SCREEN, "    pub fn handle_component_clicked(",
                     "    // no-op control\n    pub fn handle_component_clicked(",
                     "SURVIVED", "NO-OP CONTROL: a comment"),
            (SCREEN, """        if allow_insertions {
            if let Some(insertion) = clicked.insertion() {""",
                     """        if allow_insertions {
            if let Some(insertion) = clicked.insertion().filter(|_| clicked.click().is_none()) {""",
                     "KILLED", "shift PREFERS the insertion instead of replacing the click path"),
            (SCREEN, "            return ChatClick::NotHandled;\n        }\n        let Some(event) = clicked.click() else {",
                     "        }\n        let Some(event) = clicked.click() else {",
                     "KILLED", "shift falls through and runs the command anyway"),
            (SCREEN, "    command.strip_prefix('/').unwrap_or(command)",
                     "    command.trim_start_matches('/')",
                     "KILLED", "trimOptionalPrefix strips every slash"),
            (SCREEN, "                self.input.set_value(command);\n                ChatClick::Handled",
                     "                self.input.insert_text(command);\n                ChatClick::Handled",
                     "KILLED", "suggest_command inserts instead of replacing"),
            (SCREEN, "        if button != 0 {\n            return ChatClick::NotHandled;\n        }",
                     "        if button > 2 {\n            return ChatClick::NotHandled;\n        }",
                     "KILLED", "any mouse button looks for a link"),
            (SCREEN, "                self.input.insert_text(&insertion);",
                     "                self.input.set_value(&insertion);",
                     "KILLED", "the shift insertion replaces instead of inserting"),
            (SCREEN, "            ClickEvent::RunCommand(command) => {\n                // `clickCommandAction`",
                     "            ClickEvent::RunCommand(command) if false => {\n                // `clickCommandAction`",
                     "KILLED", "run_command falls through to the log-only default"),
        ],
    ),
    "f": (
        "the chat box geometry and the inverse pose",
        f"{W} chat::",
        600,
        [
            (CHAT, "    pub fn text_top(&self, line_index: i32) -> f32 {",
                   "    // no-op control\n    pub fn text_top(&self, line_index: i32) -> f32 {",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (CHAT, "        self.entry_bottom(line_index) - self.to_message_y",
                   "        self.entry_bottom(line_index) - self.entry_height",
                   "KILLED", "textTop uses entryHeight rather than entryBottomToMessageY"),
            (CHAT, "        (mouse_px.0 / gui_px).floor() / scale - MESSAGE_INDENT as f32,",
                   "        (mouse_px.0 / gui_px).floor() / scale,",
                   "KILLED", "the inverse pose drops the MESSAGE_INDENT translate"),
            (CHAT, "            chat_bottom: ((screen_h / chat_px) - BOTTOM_MARGIN as f32).floor(),",
                   "            chat_bottom: (screen_h / chat_px).floor(),",
                   "KILLED", "the bottom margin is not subtracted"),
            (CHAT, "        finder.accept(&line.text, 0.0, geom.text_top(line.index), test, width_of);",
                   "        finder.accept(&line.text, 0.0, geom.text_top(0), test, width_of);",
                   "KILLED", "every row is laid out at the bottom row's y"),
        ],
    ),
    "g": (
        "the platform opener",
        "cargo test -q -p rewo-app --bins uri_open",
        900,
        [
            (URI, "pub fn open_uri_args(uri: &str) -> Vec<String> {",
                  "pub fn open_uri_args(uri: &str) -> Vec<String> { // no-op control",
                  "SURVIVED", "NO-OP CONTROL: a comment"),
            (URI, "    if rewo_world::chat_events::parse_untrusted_uri(uri).is_none() {",
                  "    if false {",
                  "KILLED", "the second gate is gone (anything is opened)"),
            (URI, '            "url.dll,FileProtocolHandler".into(),\n            uri,',
                  '            format!("url.dll,FileProtocolHandler {uri}"),',
                  "KILLED", "the uri is folded into the middle argument"),
        ],
    ),
}


def main():
    names = sys.argv[1:]
    if not names or names == ["all"]:
        names = list(BATTERIES)
    bad = 0
    for n in names:
        name, check, timeout, muts = BATTERIES[n]
        bad += battery(f"{n}: {name}", check, timeout, muts)
    print("\nUNEXPECTED TOTAL:", bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
