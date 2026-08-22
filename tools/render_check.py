"""Stand up a FRESH vanilla test server and run `rewo live --render-check`.

    python tools/render_check.py [--keep] [--username NAME]

Written down because the recipe is the expensive part, not the gate: every trap
below has cost a previous session a run or an hour, and they are all recorded in
AGENT_LOOP_BRIEF.md's "Test server" section.

* **A fresh DIRECTORY, not a shared one.** Unlocked recipes persist into the
  save, and r25 needs a multi-page recipe book — 2026-08-07's M105b looked green
  against a world where an earlier run's `recipe give` had already landed.
  `libraries/` and `versions/` are copied because they are the server jar's own
  extraction cache; `world/`, `logs/` and `usercache.json` are not.
* **PROBE the port.** M111 lost a run to an invented one: the server died on
  FAILED TO BIND and 27 of 28 witnesses passed anyway, because most inject their
  own scenes and only r25 needs a live server. So this binds a socket to pick
  the port AND greps the log before trusting the run.
* **`eula.txt` is copied byte-for-byte.** Writing it from PowerShell gives it a
  UTF-8 BOM that the server rejects.
* **The server stops on stdin EOF**, so it cannot be backgrounded from a shell.
  This holds the pipe open for the life of the run and sends `stop` at the end.
* **The op name must be the name you connect with**, with that name's offline
  UUID, or every REWO_PRECMD command is silently rejected and it reads as a
  client bug.
* **Two caller requirements.** r14 needs items in the hotbar and r25 needs a
  multi-page recipe book, so REWO_PRECMD below grants both. A bare run scores
  17/18 and is not a pass.
* Validation is `cfg!(debug_assertions)`-gated for `live`, so this insists on the
  DEBUG binary: in a release build r17 is false and r18 is vacuous.
* **It stages the two BLOCKING configuration tasks** (M166): `resource-pack`,
  `resource-pack-id` and `enable-code-of-conduct` plus a `codeofconduct/`
  directory. Unlike every other staging here this one is not about reaching a
  witness -- it is about reaching the run at all. Without M166's replies the
  server's configuration queue never advances, the client sits in
  `run_configuration` answering keep-alives, and the gate scores **nothing**.
* **It BUILDS.** It used only to check the binary existed, so a source change
  not followed by a manual `cargo build` was graded against the previous
  compile — which is how M151's first attempt to prove r47 non-vacuous reported
  a mutation as SURVIVED.
"""
import argparse
import hashlib
import io
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", "26.2")
SRC = os.path.join(BASE, "testserver")
REWO = os.path.join(ROOT, "target", "debug", "rewo.exe")
PRECMD = (
    "give @s minecraft:diamond_sword 1;"
    "give @s minecraft:dirt 64;"
    "recipe give @s *;"
    # M168 -- r57 / r58. An iron chestplate is 6 armour (three full icons);
    # the effect is INFINITE so it never reaches the 200-tick fade, and
    # `true` hides the particles while keeping the icon (showIcon).
    "item replace entity @s armor.chest with minecraft:iron_chestplate;"
    "effect give @s minecraft:speed infinite 0 true"
)

# M166 -- the two BLOCKING configuration tasks, staged so the gate exercises the
# hang it fixes. `addOptionalTasks` queues a code-of-conduct task and a
# resource-pack task, neither of which finishes itself, so before M166 a server
# with either of these set left the client in `run_configuration` forever: no
# window, no error, and the 30s socket timeout unreachable because a keep-alive
# lands every 15s. With the fix missing this run scores **0 witnesses**, not 54.
#
# The server never fetches the URL -- `getServerPackInfo` only checks it is
# non-empty -- so an unroutable one is fine and keeps the run offline.
PACK_ID = "0f1e2d3c-4b5a-4988-8776-655443322110"
PACK_URL = "http://127.0.0.1:1/rewo-render-check.zip"
# Single line, ASCII, no section signs: the server joins the file's lines with a
# newline and runs `StringUtil.stripColor` over the result, so anything else
# would make the staged string and the delivered one disagree for reasons that
# have nothing to do with the client.
COC_TEXT = "Be excellent to each other."


def offline_uuid(name):
    """`UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))` —
    an MD5 UUID with the version and variant nibbles forced."""
    b = bytearray(hashlib.md5(("OfflinePlayer:" + name).encode("utf-8")).digest())
    b[6] = (b[6] & 0x0F) | 0x30
    b[8] = (b[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(b)))


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--keep", action="store_true", help="leave the server dir behind")
    ap.add_argument("--username", default="RewoBot")
    args = ap.parse_args()

    # **Build first.** This used to only check that the binary EXISTED, which
    # made every run grade whatever was last compiled. M151 lost a mutation
    # verification to it: `keys.tab_list` was replaced with a literal `true`,
    # the gate reported the unmutated ~50% frame count, and the mutation read
    # as SURVIVED — the same shape as the leftover-mutant-binary hazard the
    # `*_mutate.py` batteries carry a rebuild for, running the other way round.
    b = subprocess.run(["cargo", "build", "-p", "rewo-app"], cwd=ROOT)
    if b.returncode != 0:
        sys.exit("cargo build failed -- nothing below would be about this tree")
    if not os.path.exists(REWO):
        sys.exit("no debug binary at %s after a successful build" % REWO)
    port = free_port()
    dest = os.path.join(BASE, "testserver-rc-%d" % port)
    if os.path.exists(dest):
        sys.exit("%s already exists; a reused directory is not a fresh one" % dest)

    os.makedirs(dest)
    for f in ("server.jar", "eula.txt"):
        shutil.copyfile(os.path.join(SRC, f), os.path.join(dest, f))
    for d in ("libraries", "versions"):
        if os.path.isdir(os.path.join(SRC, d)):
            shutil.copytree(os.path.join(SRC, d), os.path.join(dest, d))
    assert io.open(os.path.join(dest, "eula.txt"), "rb").read().startswith(b"eula=true"), "eula"

    props = io.open(os.path.join(SRC, "server.properties"), "rb").read().decode("utf-8")
    out, hits = [], 0
    # M166 stages three keys. They are REPLACED IN PLACE and each must be hit
    # exactly once: a vanilla server writes its whole default property set, so
    # all three are already present and appending would leave two copies with
    # the server reading whichever comes last -- the gate would then grade a
    # value it did not choose. The hit count also fails loud if a key is ever
    # dropped from the template, which is the direction that would silently
    # un-stage the run.
    staged = {
        "resource-pack": PACK_URL,
        "resource-pack-id": PACK_ID,
        "enable-code-of-conduct": "true",
    }
    staged_hits = dict.fromkeys(staged, 0)
    for line in props.splitlines():
        if line.startswith("server-port="):
            line = "server-port=%d" % port
            hits += 1
        key = line.split("=", 1)[0]
        if key in staged:
            line = "%s=%s" % (key, staged[key])
            staged_hits[key] += 1
        out.append(line)
    assert staged_hits == dict.fromkeys(staged, 1), (
        "M166 staging did not land exactly once per key: %r" % staged_hits
    )
    assert hits == 1, "expected exactly one server-port line, found %d" % hits
    io.open(os.path.join(dest, "server.properties"), "wb").write(
        ("\n".join(out) + "\n").encode("utf-8")
    )
    # `enable-code-of-conduct=true` with no `codeofconduct/` directory is a
    # startup THROW, not a warning -- so this file is part of the staging, not a
    # nicety.
    os.makedirs(os.path.join(dest, "codeofconduct"))
    io.open(os.path.join(dest, "codeofconduct", "en_us.txt"), "wb").write(
        (COC_TEXT + chr(10)).encode("utf-8")
    )
    io.open(os.path.join(dest, "ops.json"), "wb").write(
        json.dumps(
            [{
                "uuid": offline_uuid(args.username),
                "name": args.username,
                "level": 4,
                "bypassesPlayerLimit": True,
            }],
            indent=2,
        ).encode("utf-8")
    )

    log = io.open(os.path.join(dest, "server-run.log"), "wb")
    print("starting server on port %d in %s" % (port, dest))
    srv = subprocess.Popen(
        ["java", "-Xmx2G", "-jar", "server.jar", "nogui"],
        cwd=dest,
        stdin=subprocess.PIPE,  # held open: the server stops on stdin EOF
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    code = None
    try:
        deadline = time.time() + 180
        ready = False
        while time.time() < deadline:
            time.sleep(2)
            txt = io.open(os.path.join(dest, "server-run.log"), "rb").read().decode("utf-8", "replace")
            if "FAILED TO BIND" in txt:
                sys.exit("server FAILED TO BIND on %d -- the probe lost a race" % port)
            if srv.poll() is not None:
                sys.exit("server exited early with %s" % srv.returncode)
            if 'Done (' in txt:
                ready = True
                break
        if not ready:
            sys.exit("server never printed Done( within 180s")
        print("server up; running render-check as %s" % args.username)

        env = dict(os.environ)
        env["REWO_PRECMD"] = PRECMD
        # One source of truth for both halves of r55/r56: the same literals
        # that went into server.properties are handed to the client, so the
        # witness compares what came off the WIRE against what the server was
        # configured with. Two hard-coded copies would drift; this cannot.
        env["REWO_RC_PACK_ID"] = PACK_ID.replace("-", "")
        env["REWO_RC_COC"] = COC_TEXT
        # TIMEOUT, because the client's worst failure mode is not a crash.
        # A configuration-state packet the client fails to answer leaves it
        # reading keep-alives forever (M166), and an untimed `run` inherits
        # that: the gate hangs with no output, no exit code and nothing to
        # diagnose. Same family as the mutation-harness hang M138d records,
        # where a hung child took the harness down and left a mutation on disk.
        # A healthy run is ~15s; the ceiling only has to be well clear of a
        # cold asset bake.
        try:
            p = subprocess.run(
                [REWO, "live", "--render-check",
                 "--host", "127.0.0.1", "--port", str(port),
                 "--username", args.username],
                cwd=ROOT,
                env=env,
                capture_output=True,
                timeout=float(os.environ.get("REWO_RC_TIMEOUT", "300")),
            )
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b"") + (e.stderr or b"")
            io.open(os.path.join(tempfile.gettempdir(), "rewo-render-check.out"),
                    "wb").write(out)
            sys.exit(
                "the client did not exit within the timeout -- it is HUNG, not "
                "slow. The usual cause is a configuration-state packet nothing "
                "answers, which stalls the server's task queue while keep-alives "
                "keep the socket alive (see crates/rewo-net/src/config_tasks.rs)."
            )
        code = p.returncode
        text = (p.stdout + p.stderr).decode("utf-8", "replace")
        # **Write bytes, do not `print` this.** The gate's rows contain arrows
        # and em dashes, and Python's stdout on Windows is cp1252 — printing it
        # raises UnicodeEncodeError *after* the run has succeeded, which loses
        # the exit code and reads as a failed gate. Same family as M95's
        # mutation harness, where a locale-decoded em dash made every verdict
        # read KILLED.
        # In TEMP, not the repo: a tool that drops an untracked artifact into
        # the working tree makes `git status` lie about whether a run left
        # anything behind, and that check is worth more than the convenience.
        report = os.path.join(tempfile.gettempdir(), "rewo-render-check.out")
        io.open(report, "wb").write(text.encode("utf-8"))
        sys.stdout.buffer.write(text[-9000:].encode("utf-8", "replace"))
        sys.stdout.flush()
        print("\nRENDER_CHECK_EXIT=%s  (full output in %s)" % (code, report))
    finally:
        try:
            srv.stdin.write(b"stop\n")
            srv.stdin.flush()
            srv.stdin.close()
            srv.wait(timeout=60)
        except Exception:
            srv.kill()
        log.close()
        if not args.keep:
            shutil.rmtree(dest, ignore_errors=True)
            print("removed %s" % dest)
        else:
            print("kept %s" % dest)
    sys.exit(code if code is not None else 1)


if __name__ == "__main__":
    main()
