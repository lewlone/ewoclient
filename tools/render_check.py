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
    "recipe give @s *"
)


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
    for line in props.splitlines():
        if line.startswith("server-port="):
            line = "server-port=%d" % port
            hits += 1
        out.append(line)
    assert hits == 1, "expected exactly one server-port line, found %d" % hits
    io.open(os.path.join(dest, "server.properties"), "wb").write(
        ("\n".join(out) + "\n").encode("utf-8")
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
        p = subprocess.run(
            [REWO, "live", "--render-check",
             "--host", "127.0.0.1", "--port", str(port),
             "--username", args.username],
            cwd=ROOT,
            env=env,
            capture_output=True,
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
