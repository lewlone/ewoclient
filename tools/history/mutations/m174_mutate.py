"""M174's mutation battery - the sign editor.

    python tools/m174_mutate.py [lo] [hi]

Checkers per mutation (M158's rule): `gate` = `rewo signshot --check` (the
model + derivation + pixel witnesses), `world` = the `sign`-filtered
rewo-world unit tests (the model's own tests + the set_sign_messages echo),
`net` = the `sign_update`-filtered rewo-net tests (the packet encoder).
The live open_sign_editor chain is not mutated here (needs a server).
`reap()` before every build (linker 1104); a timeout is a KILL; restore is
verified by BYTES; the no-op control must SURVIVE.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
MODEL = os.path.join(ROOT, "crates", "rewo-world", "src", "sign_edit_screen.rs")
LIVE = os.path.join(ROOT, "crates", "rewo-app", "src", "live_cmd.rs")
STATES = os.path.join(ROOT, "crates", "rewo-data", "src", "sign_states.rs")
BES = os.path.join(ROOT, "crates", "rewo-world", "src", "block_entities.rs")
NET = os.path.join(ROOT, "crates", "rewo-net", "src", "lib.rs")

SEC = chr(0x00A7)
STRAYS = ("rewo.exe", "rewo_world-*.exe", "rewo_net-*.exe", "rewo_gpu-*.exe")

MUTATIONS = [
    ("control: no change", MODEL,
     "pub const SIGN_LINE_HEIGHT: i32 = 10;",
     "pub const SIGN_LINE_HEIGHT: i32 = 10;",
     ["gate"], "MUST SURVIVE"),

    # ---- the field -------------------------------------------------------
    ("the blink shortens its half-period", MODEL,
     "(ms_since_open / 300) % 2 == 0",
     "(ms_since_open / 250) % 2 == 0",
     ["gate"], "TextCursorUtils blinks 300 on / 300 off; m1 pins both boundaries"),
    ("Up stops wrapping to line 3", MODEL,
     "            self.line = (self.line + 3) & 3;",
     "            self.line = self.line;",
     ["gate"], "Up is (line - 1) & 3 - from line 0 it lands on the LAST row"),
    ("Enter no longer moves down", MODEL,
     "if input.key == KEY_DOWN || input.key == KEY_ENTER || input.key == KEY_KP_ENTER {",
     "if input.key == KEY_DOWN || input.key == KEY_KP_ENTER {",
     ["gate"], "Down OR Enter OR keypad-Advance advances; Enter never CLOSES"),
    ("the width validator turns strict", MODEL,
     "if width_fn(&candidate) <= self.kind.max_text_line_width() {",
     "if width_fn(&candidate) < self.kind.max_text_line_width() {",
     ["gate"], "font.width(s) <= max: a candidate of EXACTLY the max fits"),
    ("a failed paste keeps the selection armed", MODEL,
     "            self.insert_text(&text, width_fn);\n            self.selection = self.cursor;\n            return true;",
     "            self.insert_text(&text, width_fn);\n            return true;",
     ["gate"], "EQUIVALENT (kept, labelled): insert_text collapses the selection on BOTH its paths, so the paste arm's own collapse is dead code in vanilla too - the battery records it rather than pretending a witness can kill it"),
    ("nothing is ever stripped from a paste", MODEL,
     "        if c == '" + SEC + "' {",
     "        if false {",
     ["gate"], "stripFormatting removes section-codes before an insert"),
    ("Delete reports handled", MODEL,
     "key::DELETE => {\n                self.remove_from_cursor(1, word);\n                false\n            }",
     "key::DELETE => {\n                self.remove_from_cursor(1, word);\n                true\n            }",
     ["gate"], "vanilla's case 261 has NO return true - Delete also falls through to super"),
    ("every typed character inserts", MODEL,
     "if is_allowed_chat_character(ch) {",
     "if true {",
     ["gate"], "isAllowedChatCharacter rejects 167 and control chars at the CHAR path"),
    ("the forward word-scan stops on the separator run", MODEL,
     "                    while result < b.len() && sep(result) {\n                        result += 1;\n                    }",
     "",
     ["gate"], "getWordPosition steps PAST the separator run (fwd from 0 lands at 4, not 2)"),

    # ---- the render ------------------------------------------------------
    ("the wall board grows its post back", LIVE,
     "let rows = if kind == SignKind::Wall { 12.0 } else { 26.0 };".replace("SignKind::Wall { 12.0 }", "SignKind::Wall { 12.0 }"),
     None,
     None, "SKIP-SHAPED - replaced below"),
    ("the standing board crops like a wall one", LIVE,
     "SignKind::Standing => (Sheet::SignBoard(wood), Fill::Stretch),",
     "SignKind::Standing => (Sheet::SignBoard(wood), Fill::SubRect(0, 0, 24, 12)),",
     ["gate"], "a standing sign blits all 26 rows; cropping it loses the lower probes"),
    ("the dark text scale rounds up", LIVE,
     "scale_rgb(dye, 0.4)",
     "scale_rgb(dye, 0.5)",
     ["gate"], "getDarkColor is the dye at 40%, TRUNCATING - red reads 0x660000"),

    # ---- the production derivation -------------------------------------- -
    ("the hanging suffix is not trimmed off the wood", STATES,
     '                .trim_end_matches("_hanging")',
     "",
     ["gate"], "spruce_hanging_sign -> wood `spruce`; leaving the suffix maps it to oak"),
    ("hanging signs are never detected", STATES,
     'let hanging = short.contains("hanging_sign");',
     "let hanging = false;",
     ["gate"], "HangingSignBlockEntity chooses HangingSignEditScreen - line height, width AND sheet"),

    # ---- the echo and the wire ------------------------------------------
    ("the local echo writes nothing", BES,
     "*m = messages;",
     "let _ = &messages;",
     ["world"], "setMessage replaces the edited face's messages in place"),
    ("the packet drops the front-text flag", NET,
     "w.bool(is_front_text);",
     "",
     ["net"], "ServerboundSignUpdatePacket is pos + isFrontText + four strings; dropping the flag desyncs every read after it"),
]

# The wall-rows mutation needs the model file, not live_cmd.
MUTATIONS[10] = (
    "the wall board grows its post back", MODEL,
    "let rows = if kind == SignKind::Wall { 12.0 } else { 26.0 };",
    "let rows = 26.0;",
    ["gate"],
    "PlainSignBlock.getAttachmentPoint(state) == WALL ? 12 : 26 - a wall board that draws 26 rows puts the POST on screen",
)


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
    return 0, "ok"


CHECKERS = {
    "gate": lambda: run([EXE, "signshot", "--check"], 600),
    "world": lambda: unit("rewo-world", "sign"),
    "net": lambda: unit("rewo-net", "sign_update"),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in (m[4] or [])}) or ["gate"]
    print(f"[m174] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control; checkers {used}")
    results = []
    for name, path, find, repl, checkers, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        is_control = name.startswith("control")
        verdict, reason = None, ""
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
            if "gate" in checkers and not build():
                # A mutation that breaks the build is a KILL for a mutation
                # (the gate cannot even run) but fatal for the control.
                if is_control:
                    verdict, reason = "FAILED", "control build failed"
                else:
                    verdict, reason = "KILLED", "build failed"
            else:
                for c in checkers:
                    code, r = CHECKERS[c]()
                    if code is None:
                        verdict, reason = "TIMEOUT", f"{c}"
                        break
                    if code != 0:
                        reason = f"killed by {c} ({r.strip().splitlines()[-1][:120] if r.strip() else 'no output'})"
                        break
                else:
                    reason = "all checkers green"
            if verdict is None:
                if is_control:
                    verdict = "SURVIVED" if code == 0 and not reason.startswith("killed") else "FAILED"
                    if reason.startswith("killed"):
                        verdict = "FAILED"
                else:
                    verdict = "KILLED" if reason.startswith("killed") else "SURVIVED"
        finally:
            with io.open(path, "w", encoding="utf-8", newline="") as f:
                f.write(original)
            after = io.open(path, encoding="utf-8", newline="").read()
            if after != original:
                print(f"RESTORE FAILED for {path} - STOPPING THE BATTERY")
                sys.exit(2)
        results.append((name, verdict, reason))
        print(f"{verdict:9} {name}: {reason}")
    killed = sum(1 for _, v, _ in results if v == "KILLED")
    controls_ok = sum(1 for n, v, _ in results if n.startswith("control") and v == "SURVIVED")
    bad = [n for n, v, _ in results if v in ("SURVIVED", "FAILED", "TIMEOUT") and not n.startswith("control")]
    print(f"[m174] {killed} killed, control {'ok' if controls_ok else 'FAILED'}, "
          f"{len(bad)} survived: {[b for b in bad]}")


if __name__ == "__main__":
    main()
