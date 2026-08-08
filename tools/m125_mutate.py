"""M125's mutation harness.

Verdicts come from the EXIT CODE of the check, never from a substring of its
output: M109 grepped for witness names, saw `ok` on every line, and missed that
the command was red — and then a whole battery run against an already-failing
command read KILLED for every entry.

Every battery must contain a NO-OP CONTROL that is expected to SURVIVE. A
battery whose control dies is measuring a broken instrument, not the code.

    python tools/m125_mutate.py <battery>
"""

import os
import subprocess
import sys

ROOT = "."

# Set on the ENVIRONMENT, not as a `VAR=value cmd` prefix. `shell=True` on
# Windows runs cmd.exe, where that prefix is not syntax at all: cmd tries to
# execute a program literally named `REWO_PRECMD="give`, the check never runs,
# and the harness reports the baseline red. Which it did, and which is what the
# baseline guard is for.
CHECK_ENV = {
    "REWO_PRECMD": "give @s minecraft:diamond_sword 1;"
                   "give @s minecraft:dirt 64;recipe give @s *",
}


def run(cmd, timeout):
    env = dict(os.environ)
    env.update(CHECK_ENV)
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True,
                           timeout=timeout, env=env)
        return p.returncode, (p.stdout + p.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        # A mutation that makes the check hang is killed by it. That is the
        # only verdict available and it is the right one — an unbounded walk
        # is exactly what the budget exists to stop.
        return 124, "<timed out>"


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
        full = f"{ROOT}/{path}"
        orig = open(full, "rb").read()
        n = orig.decode("utf-8").count(old)
        if n != 1:
            print(f"  ?? {why}\n     anchor matched {n} times, not 1 — SKIPPED")
            bad += 1
            continue
        try:
            open(full, "wb").write(orig.decode("utf-8").replace(old, new, 1).encode("utf-8"))
            code, _ = run(check, timeout)
        finally:
            open(full, "wb").write(orig)
        got = "KILLED" if code != 0 else "SURVIVED"
        ok = got == expect
        if not ok:
            bad += 1
        print(f"  {'ok ' if ok else 'BAD'} {got:9s} (want {expect:9s})  {why}")
    print(f"    {'ALL AS EXPECTED' if bad == 0 else str(bad) + ' UNEXPECTED'}")
    return bad


LANG = "crates/rewo-data/src/lang.rs"
TRANS = "crates/rewo-net/src/chat_translate.rs"
STYLE = "crates/rewo-net/src/chat_style.rs"
NBT = "crates/rewo-proto/src/nbt.rs"

BATTERIES = {
    "a": (
        "decompose_template",
        "cargo test -q -p rewo-data --lib",
        600,
        [
            (LANG, "    let mut replacement_index = 0usize;",
                   "    let mut replacement_index = 0usize; // no-op control",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (LANG, "            if index >= args_len {",
                   "            if index > args_len {",
                   "KILLED", "an index one past the end is accepted"),
            (LANG, "            out.push(Part::Literal(&template[m.start..m.start + 1]));",
                   "            out.push(Part::Literal(&template[m.start..m.start + 2]));",
                   "KILLED", "`%%` emits two characters, not one"),
            (LANG, """                None => {
                    let i = replacement_index;
                    replacement_index += 1;
                    i
                }""",
                   """                None => {
                    let i = replacement_index;
                    replacement_index += 1;
                    i
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),""",
                   "SURVIVED", "NO-OP CONTROL 2: an unreachable arm"),
            (LANG, "        if m.start > current {",
                   "        if m.start >= current {",
                   "KILLED", "an empty leading literal is emitted"),
        ],
    ),
    "b": (
        "the translatable's inputs and the arg toString",
        "cargo test -q -p rewo-net --lib chat_translate",
        600,
        [
            (TRANS, "    let key = tag.get(\"translate\").and_then(Nbt::as_str)?;",
                    "    let key = tag.get(\"translate\").and_then(Nbt::as_str)?; // no-op control",
                    "SURVIVED", "NO-OP CONTROL: a comment"),
            (TRANS, "        (Some(l), Some(f)) => l.get_or_default(key, f),",
                    "        (Some(l), Some(f)) => { let _ = l; f },",
                    "KILLED", "the fallback beats a present key"),
            (TRANS, "        Nbt::Int(v) => v.to_string(),",
                    "        Nbt::Int(v) => java_double_string(*v as f64),",
                    "KILLED", "an int loses its width (the createNumeric reading)"),
            (TRANS, "    if v == 0.0 || (1e-3..1e7).contains(&mag) {\n        with_point(format!(\"{v}\"))\n    } else {\n        java_scientific(format!(\"{v:e}\"))\n    }\n}\n\n/// `java.lang.Float.toString`.",
                    "    if v == 0.0 || (1e-3..=1e7).contains(&mag) {\n        with_point(format!(\"{v}\"))\n    } else {\n        java_scientific(format!(\"{v:e}\"))\n    }\n}\n\n/// `java.lang.Float.toString`.",
                    "KILLED", "the high threshold is inclusive (1e7 prints plain)"),
            (TRANS, "fn with_point(s: String) -> String {\n    if s.contains('.') {",
                    "fn with_point(s: String) -> String {\n    if true || s.contains('.') {",
                    "KILLED", "no mandatory fractional digit"),
            (TRANS, "    if v == 0.0 || (1e-3..1e7).contains(&mag) {\n        with_point(format!(\"{v}\"))\n    } else {\n        java_scientific(format!(\"{v:e}\"))\n    }\n}\n\n/// Give a plain decimal",
                    "    if v == 0.0 || (1e-3..1e7).contains(&mag) {\n        with_point(format!(\"{}\", v as f64))\n    } else {\n        java_scientific(format!(\"{:e}\", v as f64))\n    }\n}\n\n/// Give a plain decimal",
                    "KILLED", "a float's digits come from a widened f64"),
        ],
    ),
    "c": (
        "the styled walk",
        "cargo test -q -p rewo-net --lib chat_style",
        600,
        [
            (STYLE, "                None => walk(&t.args[i], style, lang, steps, out),",
                    "                None => walk(&t.args[i], ChatStyle::WHITE, lang, steps, out),",
                    "KILLED", "a component argument does not inherit the template's style"),
            (STYLE, "        push_legacy(t.template, style, style, out);\n        return;",
                    "        return;",
                    "KILLED", "a format error renders nothing instead of the template"),
            (STYLE, "            } else if let Some(t) = crate::chat_translate::translatable(tag, lang) {\n                walk_translatable(&t, style, lang, steps, out);",
                    "            } else if let Some(t) = crate::chat_translate::translatable(tag, None) {\n                walk_translatable(&t, style, lang, steps, out);",
                    "KILLED", "the table is dropped at the lookup (the pre-M125 path)"),
            (STYLE, "                Some(text) => push_legacy(&text, style, style, out),",
                    "                Some(text) => push_legacy(&text, style, style, out), // no-op control",
                    "SURVIVED", "NO-OP CONTROL: a comment"),
        ],
    ),
    "d": (
        "the NBT list unwrap",
        "cargo test -q -p rewo-proto --lib",
        600,
        [
            (NBT, "                items.push(unwrap_list_element(read_payload(r, elem_tag, depth + 1)?));",
                  "                items.push(read_payload(r, elem_tag, depth + 1)?);",
                  "KILLED", "no unwrap at all (the pre-M125e reader)"),
            (NBT, "        Nbt::Compound(entries) if entries.len() == 1 && entries[0].0.is_empty() => {",
                  "        Nbt::Compound(entries) if entries[0].0.is_empty() => {",
                  "KILLED", "isWrapper drops the size()==1 half"),
            (NBT, "        Nbt::Compound(entries) if entries.len() == 1 && entries[0].0.is_empty() => {",
                  "        Nbt::Compound(entries) if entries.len() == 1 => {",
                  "KILLED", "isWrapper drops the empty-key half"),
            (NBT, "const MAX_DEPTH: u32 = 128;",
                  "const MAX_DEPTH: u32 = 128; // no-op control",
                  "SURVIVED", "NO-OP CONTROL: a comment"),
        ],
    ),
}

# The two mutations no unit test can catch, because what they break is the
# WIRING: the table reaching the session, and the reader the session's
# component came through. Both need the windowed client and a live server, so
# the check is `live --render-check` and each entry costs a debug rebuild.
#
# The port must be a PROBED, running test server (M111's finding: a gate whose
# witnesses are mostly injected looks perfectly healthy against nothing).
LIVE_PORT = 25640
# Built with `os.path.join` rather than written as a literal, because cmd.exe
# cannot run `./target/...` at all ("'.' is not recognized") and a hand-written
# `target\debug\rewo.exe` is a `\r` in a Python string.
LIVE_EXE = os.path.join("target", "debug", "rewo.exe")
LIVE_CHECK = (
    f"cargo build -q && {LIVE_EXE} live --host 127.0.0.1 "
    f"--port {LIVE_PORT} --username RewoLive --render-check"
)
BATTERIES["live"] = (
    "the wiring, through live --render-check",
    LIVE_CHECK,
    900,
    [
        ("crates/rewo-app/src/live_cmd.rs",
         "    session.lang = Some(std::sync::Arc::new(baked.lang.clone()));",
         "    session.lang = None;",
         "KILLED", "the table never reaches the session (the pre-M125 client)"),
        (NBT,
         "                items.push(unwrap_list_element(read_payload(r, elem_tag, depth + 1)?));",
         "                items.push(read_payload(r, elem_tag, depth + 1)?);",
         "KILLED", "no NBT list unwrap — the integer argument vanishes"),
        ("crates/rewo-app/src/live_cmd.rs",
         "    session.lang = Some(std::sync::Arc::new(baked.lang.clone()));",
         "    session.lang = Some(std::sync::Arc::new(baked.lang.clone())); // no-op control",
         "SURVIVED", "NO-OP CONTROL: a comment"),
    ],
)

if __name__ == "__main__":
    which = sys.argv[1]
    name, check, timeout, muts = BATTERIES[which]
    sys.exit(1 if battery(name, check, timeout, muts) else 0)
