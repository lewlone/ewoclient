"""Load NORMAL terrain at a real render distance and check the client kept up.

    python tools/load_check.py [--rd 16] [--seconds 90] [--keep]

Every gate and `tools/render_check.py` use a flat world, whose columns are a
few hundred vertices each. This stands up a fresh vanilla server with
`level-type=minecraft:normal` (a fixed seed) at view distance `--rd`, gives
the client an `options.txt` asking for the same distance, runs the debug
`rewo live` for `--seconds`, and passes only if:

* the client exited 0 with no Vulkan validation message in its output;
* its last `world: arena ...` line (logged every 10 s by `live_cmd::view`)
  holds EXACTLY the columns a 26.2 server tracks at that distance
  (`ChunkTrackingView.isWithinDistance(.., includeNeighbors = true)`, the rule
  `render_check.rs`'s `server_tracked_columns` mirrors) -- so a client that
  asked for less, or dropped columns, fails -- with **0 deferred** (a deferred
  column is a hole in the world);
* the terrain arena never refused to grow.

`--light` (M212) runs the headless `rewo play --no-build --light-check` bot
instead, on a world of `--seed` at view distance `--rd` (8 is plenty): it
passes only if the bot's position was never corrected by the server
(`CORRECTIONS: 0`, the physics-parity meter, over natural terrain) and the
light check found no disagreement it cannot trace to the server's own data
(`play_cmd.rs` explains the two classes it excuses). It found M211's two
light bugs (huge mushroom caps and water over seagrass let sky light through).
A rare stale block-light cell can fail it (seen once in a dozen runs, not
reproduced): rerun before believing a single block-light failure.

Two bugs came from this check before it was a tool (2026-10-18): the fixed
terrain arena deferred 197 of 473 columns (M205), and the client asked every
server for view distance 10 whatever `options.txt` said (M206).

Not part of `tools/gates.py`: it needs a 3 GB server, minutes of world
generation and a window. It takes the machine-wide GPU lock like the render
check. The full client output is kept in TEMP (`rewo-load-check-<pid>.out`).
"""
import argparse
import io
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from gpu_lock import machine_lock

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", "26.2")
SRC = os.path.join(BASE, "testserver")
REWO = os.path.join(ROOT, "target", "debug", "rewo.exe")
# A fixed seed, so two runs load the same terrain (the 2026-10-18 numbers:
# ~16.2k vertices a column at render distance 16).
SEED = "20261018"
ARENA = re.compile(r"world: arena \d+/\d+ verts .* (\d+) columns, (\d+) deferred")


def tracked_columns(vd):
    """`ChunkTrackingView.isWithinDistance(cx, cz, vd, x, z, true)` over every
    offset: `max(0, |d| - 2)` per axis, squared and summed, `< vd * vd`."""
    n = 0
    for dx in range(-vd - 2, vd + 3):
        for dz in range(-vd - 2, vd + 3):
            bx, bz = max(0, abs(dx) - 2), max(0, abs(dz) - 2)
            n += bx * bx + bz * bz < vd * vd
    return n


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rd", type=int, default=16, help="server and client render distance")
    ap.add_argument("--seconds", type=int, default=90)
    ap.add_argument("--keep", action="store_true", help="leave the server dir behind")
    ap.add_argument("--light", action="store_true",
                    help="run the play bot with --light-check instead of the windowed client")
    ap.add_argument("--seed", default=SEED, help="level seed (default %s)" % SEED)
    args = ap.parse_args()
    if not 2 <= args.rd <= 32:
        sys.exit("--rd must be 2..32 (vanilla's IntRange)")
    b = subprocess.run(["cargo", "build", "-p", "rewo-app", "--features", "gates"], cwd=ROOT)
    if b.returncode != 0:
        sys.exit("cargo build failed -- nothing below would be about this tree")
    with machine_lock("tools/load_check.py"):
        sys.exit(run(args))


def run(args):
    port = free_port()
    dest = os.path.join(BASE, "testserver-load-%d" % port)
    if os.path.exists(dest):
        sys.exit("%s already exists; a reused directory is not a fresh one" % dest)
    os.makedirs(dest)
    for f in ("server.jar", "eula.txt"):
        shutil.copyfile(os.path.join(SRC, f), os.path.join(dest, f))
    for d in ("libraries", "versions"):
        if os.path.isdir(os.path.join(SRC, d)):
            shutil.copytree(os.path.join(SRC, d), os.path.join(dest, d))
    props = io.open(os.path.join(SRC, "server.properties"), "rb").read().decode("utf-8")
    want = {
        "server-port": str(port),
        "level-type": "minecraft\\:normal",
        "view-distance": str(args.rd),
        "level-seed": args.seed,
    }
    out, hits = [], {}
    for line in props.splitlines():
        key = line.split("=", 1)[0]
        if key in want:
            line = "%s=%s" % (key, want[key])
            hits[key] = hits.get(key, 0) + 1
        out.append(line)
    # Replaced in place, each exactly once (render_check.py's rule): an
    # appended duplicate would leave the server reading whichever came last.
    if hits != dict.fromkeys(want, 1):
        sys.exit("server.properties staging did not land once per key: %r" % hits)
    io.open(os.path.join(dest, "server.properties"), "wb").write(("\n".join(out) + "\n").encode())
    game = os.path.join(dest, "client-game")
    os.makedirs(game)
    io.open(os.path.join(game, "options.txt"), "wb").write(("renderDistance:%d\n" % args.rd).encode())

    log = io.open(os.path.join(dest, "server-run.log"), "wb")
    print("starting server on port %d in %s (normal world, view-distance %d)" % (port, dest, args.rd))
    srv = subprocess.Popen(["java", "-Xmx3G", "-jar", "server.jar", "nogui"], cwd=dest,
                           stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
    try:
        deadline = time.time() + 300
        while True:
            time.sleep(2)
            txt = io.open(os.path.join(dest, "server-run.log"), "rb").read().decode("utf-8", "replace")
            if "FAILED TO BIND" in txt:
                return "server FAILED TO BIND on %d" % port
            if srv.poll() is not None:
                return "server exited during startup (see %s)" % dest
            if "Done (" in txt:
                break
            if time.time() > deadline:
                return "server never became ready"
        if args.light:
            cmd = [REWO, "play", "--host", "127.0.0.1", "--port", str(port),
                   "--username", "LoadBot", "--seconds", str(args.seconds),
                   "--no-build", "--light-check"]
        else:
            cmd = [REWO, "live", "--host", "127.0.0.1", "--port", str(port),
                   "--username", "LoadBot", "--run-seconds", str(args.seconds),
                   "--game-dir", game]
        print("server up; running rewo %s for %d s" % (cmd[1], args.seconds))
        try:
            p = subprocess.run(cmd, cwd=ROOT, capture_output=True, timeout=args.seconds + 240)
        except subprocess.TimeoutExpired:
            return "the client did not exit within %d s -- it is hung" % (args.seconds + 240)
        text = (p.stdout + p.stderr).decode("utf-8", "replace")
        report = os.path.join(tempfile.gettempdir(), "rewo-load-check-%d.out" % os.getpid())
        io.open(report, "wb").write(text.encode("utf-8"))
        if args.light:
            return grade_light(text, p.returncode, report)
        return grade(text, p.returncode, args.rd, report)
    finally:
        try:
            srv.stdin.write(b"stop\n")
            srv.stdin.flush()
            srv.wait(60)
        except Exception:
            srv.kill()
        # The server's log stays open in THIS process until closed, and
        # Windows will not delete an open file: an ignore_errors removal left
        # the directory and its log behind on every run (54 of them).
        log.close()
        if not args.keep:
            shutil.rmtree(dest, ignore_errors=True)
            if os.path.exists(dest):
                print("could not remove %s" % dest)


def grade_light(text, code, report):
    """`--light`: the bot's verdict, its corrections, and the light check."""
    lines = text.splitlines()
    for l in lines:
        if "CORRECTIONS" in l or "[rewo-light]" in l or "[verdict]" in l:
            shown = l.split("] ", 1)[-1] if l.startswith("[20") else l
            sys.stdout.buffer.write(shown[:300].encode("utf-8", "replace") + b"\n")
    fails = []
    if code != 0:
        fails.append("play exit %d" % code)
    corr = [re.search(r"CORRECTIONS: (\d+)", l) for l in lines]
    corr = [int(m.group(1)) for m in corr if m]
    if not corr:
        fails.append("no CORRECTIONS line")
    elif corr[-1] != 0:
        fails.append("%d server position corrections" % corr[-1])
    if not any("[verdict] light-check: ok" in l for l in lines):
        fails.append("the light check did not pass")
    if any("VUID-" in l for l in lines):
        fails.append("validation messages")
    print("full output: %s" % report)
    if fails:
        print("LOAD CHECK FAILED: " + "; ".join(fails))
        return 1
    print("LOAD CHECK PASSED")
    return 0


def grade(text, code, rd, report):
    lines = text.splitlines()
    for l in lines:
        if "frame time:" in l or "avg fps" in l or "arena grown" in l:
            sys.stdout.buffer.write((l.split("] ", 1)[-1] + "\n").encode("utf-8", "replace"))
    fails = []
    if code != 0:
        fails.append("client exit %d" % code)
    vuids = sum(1 for l in lines if "VUID-" in l or "Validation Error" in l)
    if vuids:
        fails.append("%d validation message(s)" % vuids)
    if any("arena growth failed" in l for l in lines):
        fails.append("the terrain arena refused to grow")
    arena = [m for m in (ARENA.search(l) for l in lines) if m]
    expected = tracked_columns(rd)
    if not arena:
        fails.append("no `world: arena` line (the run was shorter than 10 s?)")
    else:
        cols, deferred = int(arena[-1].group(1)), int(arena[-1].group(2))
        print("last arena line: %d columns (expected %d at render distance %d), %d deferred"
              % (cols, expected, rd, deferred))
        if cols != expected:
            fails.append("%d columns loaded, the tracking set at %d is %d" % (cols, rd, expected))
        if deferred:
            fails.append("%d columns deferred (holes in the world)" % deferred)
    print("full output: %s" % report)
    if fails:
        print("LOAD CHECK FAILED: " + "; ".join(fails))
        return 1
    print("LOAD CHECK PASSED")
    return 0


if __name__ == "__main__":
    main()
