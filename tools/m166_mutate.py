"""M166's mutation battery — the two blocking configuration tasks.

    python tools/m166_mutate.py [lo] [hi]      # unit half, ~1 min
    python tools/m166_mutate.py --live [lo] [hi]   # wiring half, ~2 min EACH

**Two halves, because one check cannot grade both claims.** M158's gotcha 0d is
that a battery must be routed through whatever you are claiming coverage from,
and this milestone has two different things to claim:

* the ARITHMETIC — ordinals, `isTerminal`, the URL rule, the decode, the reply
  bytes — lives in `crates/rewo-net/src/config_tasks.rs` and is graded by
  `cargo test -p rewo-net --lib config_tasks`. Fast, and that is the check that
  covers it.
* the WIRING — that `run_configuration` and `handle_packet` actually CALL any of
  it — is graded by nothing in `cargo test`. Delete both dispatch arms and every
  unit test above stays green while the client hangs on every real server. Only
  `tools/render_check.py` can see that, so the `--live` half routes through it.

The live half is worth its runtime because its failure mode is unique in this
repo: the mutant does not render a wrong pixel, it **never renders at all**.
`render_check.py` grew a timeout in the same commit for exactly this reason —
without it a hung client hangs the gate, with no exit code and nothing to read.

Discipline, inherited: a no-op control that must SURVIVE, exit codes rather
than substrings, a per-mutation timeout so a hang is a KILL rather than an
outage that leaves the mutant on disk, and a restore verified by BYTES (M138a —
`git diff --quiet` cannot tell a leftover mutation from uncommitted work).
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CT = os.path.join(ROOT, "crates/rewo-net/src/config_tasks.rs")
LIB = os.path.join(ROOT, "crates/rewo-net/src/lib.rs")

# ── the arithmetic ───────────────────────────────────────────────────────────
UNIT = [
    ("control: no change", CT,
     "pub fn ordinal(self) -> i32 {", "pub fn ordinal(self) -> i32 {",
     "MUST SURVIVE — otherwise every verdict below is vacuous"),

    ("the ordinals are off by one",
     CT, "        self as i32\n", "        self as i32 + 1\n",
     "every action would name its successor; FAILED_DOWNLOAD would send "
     "ACCEPTED, which is not terminal, and the client would hang exactly as "
     "before the milestone"),

    ("isTerminal is inverted into an allow-list",
     CT,
     "        !matches!(self, PackAction::Accepted | PackAction::Downloaded)",
     "        matches!(self, PackAction::Accepted | PackAction::Downloaded)",
     "the six results would read as progress reports and the two progress "
     "reports as results — the debug_assert in `answer_pack_push` would then "
     "fire on every real push"),

    ("isTerminal answers true for everything",
     CT,
     "        !matches!(self, PackAction::Accepted | PackAction::Downloaded)",
     "        true || matches!(self, PackAction::Accepted | PackAction::Downloaded)",
     "the denial-list would stop being a claim, and a future change to "
     "ACCEPTED would pass its own assertion while hanging the client"),

    ("the reply is DECLINED",
     CT,
     "        PackAction::FailedDownload\n    } else {",
     "        PackAction::Declined\n    } else {",
     "THE user-facing regression: still terminal, still unhangs the client, "
     "and `ServerCommonPacketListenerImpl:107` disconnects you from every "
     "`require-resource-pack=true` server"),

    ("the reply is ACCEPTED",
     CT,
     "        PackAction::FailedDownload\n    } else {",
     "        PackAction::Accepted\n    } else {",
     "non-terminal: the reply is sent, the server ignores it, and the "
     "configuration queue stalls — the original bug wearing a fix"),

    ("the URL rule is a prefix test",
     CT,
     '    let Some((scheme, _)) = url.split_once(\':\') else {\n        return false;\n    };\n    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")',
     '    url.starts_with("http")',
     "`httpx://` and `httpfoo:` would read as web URLs; vanilla splits on the "
     "SCHEME, not on a prefix"),

    ("every URL is loadable",
     CT,
     '    let Some((scheme, _)) = url.split_once(\':\') else {\n        return false;\n    };\n    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")',
     "    let _ = url;\n    true",
     "INVALID_URL would never be sent; both are terminal, so this is invisible "
     "to the live gate and only a unit witness can see it"),

    ("the scheme comparison is case-sensitive",
     CT,
     '    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")',
     '    scheme == "http" || scheme == "https"',
     "`URL(String)` lowercases the scheme before its handler lookup, so "
     "`HTTP://x` is loadable in vanilla and would not be here"),

    ("the optional prompt flag is never read",
     CT,
     "    let prompt = if r.bool()? { Some(r.nbt()?) } else { None };",
     "    let prompt = None;",
     "the prompt is the LAST field, so the four fields before it still decode "
     "— the packet would look perfectly well-formed while the body went "
     "unwalked"),

    ("url and hash are read in the wrong order",
     CT,
     '    let url = r.string(32767)?;\n    let hash = r.string(40)?;',
     '    let hash = r.string(40)?;\n    let url = r.string(32767)?;',
     "two adjacent strings: the swap decodes without erroring on a short "
     "hash and puts the URL where the hash belongs"),

    ("a malformed push is answered NON-terminally",
     CT,
     '            (0, PackAction::FailedDownload)',
     '            (0, PackAction::Accepted)',
     "the error path is the one nobody exercises, and a non-terminal answer "
     "there hangs the client on exactly the packets it could not read"),

    ("the reply is not recorded",
     CT,
     "    log.pack_replies.push((id, action));",
     "    let _ = &mut log.pack_replies;",
     "r55 reads this log; an unrecorded reply makes the live witness vacuous "
     "in the direction that looks like a pass"),

    ("the accept carries a body byte",
     CT,
     "pub fn write_code_of_conduct_accept(packet_id: i32) -> PacketWriter {\n    PacketWriter::packet(packet_id)",
     "pub fn write_code_of_conduct_accept(packet_id: i32) -> PacketWriter {\n    let mut p = PacketWriter::packet(packet_id);\n    p.u8(0);\n    p",
     "`StreamCodec.unit` — one stray byte desynchronises the server's reader "
     "for every packet after it"),

    ("the reply writes the action before the id",
     CT,
     "    p.uuid(id);\n    p.varint(action.ordinal());",
     "    p.varint(action.ordinal());\n    p.uuid(id);",
     "same 17 bytes, transposed: the server reads a UUID out of the action "
     "byte and 15 bytes of the id"),
]

# ── the wiring ───────────────────────────────────────────────────────────────
# Each of these leaves `config_tasks` untouched and correct. Every unit test
# above passes. The client hangs.
LIVE = [
    ("control: no change", LIB,
     "                x if x == self.ids.cb_config_code_of_conduct => {",
     "                x if x == self.ids.cb_config_code_of_conduct => {",
     "MUST SURVIVE — otherwise every verdict below is vacuous"),

    ("the code-of-conduct arm never fires",
     LIB,
     "                x if x == self.ids.cb_config_code_of_conduct => {",
     "                x if false => {",
     "the FIRST blocking task. The client never reaches play, so the gate "
     "scores nothing at all — not a failed row, a failed RUN"),

    ("the resource-pack arm never fires",
     LIB,
     "                x if x == self.ids.cb_config_resource_pack_push => {",
     "                x if false => {",
     "the second blocking task, reached only once the first is answered"),

    # Its first version did not COMPILE, and a BUILD-FAIL is not a kill: the
    # mutant never ran, so the verdict said nothing about any witness (M141h —
    # a battery can only grade claims that survive compilation). Its second
    # anchored on `self.send(ack)?;`, which occurs FIVE times in lib.rs and so
    # was reported as a SKIP by the anchor-count guard. This one builds, is
    # unique, and drops only the send.
    ("the code of conduct is decoded but never answered",
     LIB,
     "                        self.ids.sb_config_accept_code_of_conduct,\n"
     "                    );\n"
     "                    self.send(ack)?;",
     "                        self.ids.sb_config_accept_code_of_conduct,\n"
     "                    );\n"
     "                    let _ = ack;",
     "the subtlest shape: the packet is read, the log is written, and nothing "
     "goes back — which is exactly what an implementation that treats a "
     "blocking task as a plain decode looks like, and it hangs identically"),
]


def run_unit():
    try:
        p = subprocess.run(
            ["cargo", "test", "-p", "rewo-net", "--lib", "config_tasks"],
            cwd=ROOT, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=420,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    if p.returncode != 0:
        why = ("build failed"
               if "error[E" in p.stderr or "could not compile" in p.stderr
               else "tests failed")
        return False, why
    if "test result: ok" not in p.stdout:
        return False, "no test result line"
    return True, "passed"


def run_live():
    env = dict(os.environ)
    # A healthy client finishes in ~15s. A hung one never does, and the point of
    # this half is that the difference is total rather than marginal.
    env.setdefault("REWO_RC_TIMEOUT", "90")
    try:
        p = subprocess.run(
            [sys.executable, os.path.join(ROOT, "tools", "render_check.py")],
            cwd=ROOT, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=600, env=env,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    if p.returncode != 0:
        tail = (p.stdout + p.stderr).strip().splitlines()
        return False, tail[-1][:120] if tail else "non-zero exit"
    return True, "gate green"


def main():
    argv = sys.argv[1:]
    live = "--live" in argv
    argv = [a for a in argv if a != "--live"]
    table, runner, label = (LIVE, run_live, "live") if live else (UNIT, run_unit, "unit")

    lo = int(argv[0]) if argv else 1
    hi = int(argv[1]) if len(argv) > 1 else len(table)
    selected = [table[0]] + table[lo:hi]
    print(f"[m166/{label}] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control")

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
                original.replace(find, repl, 1)
            )
            survived, reason = runner()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
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
