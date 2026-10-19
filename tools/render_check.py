"""Stand up a FRESH vanilla test server and run `rewo live --render-check`.

    python tools/render_check.py [--keep] [--username NAME]

Written down because the recipe is the expensive part, not the gate: every trap
below has cost a previous session a run or an hour, and they are all recorded in
docs/history/AGENT_LOOP_BRIEF.md's "Test server" section.

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

from gpu_lock import machine_lock
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
    # the effect is INFINITE so it never reaches the 200-tick fade. The
    # `hideParticles` argument is deliberately NOT passed: `/effect give`
    # builds `new MobEffectInstance(effect, duration, amplifier, false,
    # particles)` (EffectCommands.java:186), and that five-argument
    # constructor is `this(.., visible, visible)` (MobEffectInstance.java:61)
    # -- so hiding the particles hides the ICON too. The first cut staged
    # `... 0 true` and r58 correctly measured zero icons.
    "item replace entity @s armor.chest with minecraft:iron_chestplate;"
    "effect give @s minecraft:speed infinite 0;"
    # M194 -- r71. A real item frame from the server, so its metadata slots
    # (8 DIRECTION, 9 ITEM_STACK, 10 INT) are calibrated against 26.2 rather
    # than against the decompile's count. `Fixed` keeps it from popping off
    # with nothing behind it; facing 2 and rotation 3 are both non-default,
    # so the server must send all three slots.
    # M179 -- r74. Two root advancements, so the advancements screen has two
    # tabs and a click on the one it did not open on is a CHANGE, which is
    # the only thing `PlayerAdvancements.setSelectedTab` echoes.
    # M184 -- r75. A lectern holding a two-page written book two blocks from
    # the bot; the client right-clicks it, turns a page, and closes it.
    "setblock ~2 ~ ~ minecraft:lectern[has_book=true]{Book:{id:\"minecraft:written_book\","
    "count:1,components:{\"minecraft:written_book_content\":{title:\"rc\",author:\"rc\","
    "pages:[\"one\",\"two\"]}}}};"
    # M216 -- r76. A librarian that cannot wander (NoAI) three blocks from
    # the bot, with two known trades: the client right-clicks it and the
    # decoded `merchant_offers` must be exactly these.
    "summon minecraft:villager ~ ~ ~3 {NoAI:1b,VillagerData:{profession:\"minecraft:librarian\","
    "level:2,type:\"minecraft:plains\"},Offers:{Recipes:["
    "{buy:{id:\"minecraft:emerald\",count:5},sell:{id:\"minecraft:book\",count:1},maxUses:12},"
    "{buy:{id:\"minecraft:paper\",count:24},sell:{id:\"minecraft:emerald\",count:1},maxUses:16}"
    "]}};"
    # M217 -- r78. Persistent oak leaves two blocks from the bot (destroy time
    # 0.2, no tool needed; leaves, unlike a carpet, survive the water r53's
    # waterlogged slab spreads around spawn): the client mines them in
    # survival.
    "setblock ~ ~ ~-2 minecraft:oak_leaves[persistent=true];"
    # M219 -- r83. A bed two blocks out (foot, then head to its west): the
    # client sleeps in it at night and leaves through the in-bed screen.
    "setblock ~-2 ~ ~2 minecraft:red_bed[facing=west,part=foot];"
    "setblock ~-3 ~ ~2 minecraft:red_bed[facing=west,part=head];"
    # M220 -- r84 / r85. A tamed, saddled donkey with a chest (five columns)
    # and three apples at chest index 7, standing still: the client opens its
    # inventory, rides it, and opens it again with the inventory key. West of
    # the bot: the item frame below hangs in the bot's head block on its
    # north side, and a ray to anything north-east crosses it first.
    "summon minecraft:donkey ~-3 ~ ~-1 {Tame:1b,ChestedHorse:1b,NoAI:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}},"
    "Items:[{Slot:7b,id:\"minecraft:apple\",count:3}]};"
    "advancement grant @s only minecraft:story/root;"
    "advancement grant @s only minecraft:adventure/root;"
    "summon minecraft:item_frame ~ ~1 ~ "
    "{Facing:2b,Fixed:1b,ItemRotation:3b,Item:{id:\"minecraft:diamond_sword\",count:1}}"
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



def report_path():
    """Where this run's full output goes: per process, so two runs (or a
    stale file from an earlier run) can never be mistaken for this one."""
    return os.path.join(tempfile.gettempdir(),
                        "rewo-render-check-%d.out" % os.getpid())

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
    b = subprocess.run(["cargo", "build", "-p", "rewo-app", "--features", "gates"], cwd=ROOT)
    if b.returncode != 0:
        sys.exit("cargo build failed -- nothing below would be about this tree")
    if not os.path.exists(REWO):
        sys.exit("no debug binary at %s after a successful build" % REWO)
    # One GPU verification per machine (tools/gpu_lock.py): a second render
    # check, or a gate sweep, on the same desktop would share the GPU, the
    # window focus and this script's report path.
    with machine_lock("tools/render_check.py"):
        run(args)


# M206 -- r73: the client's staged render distance. Below the test server's
# view-distance (8), so the server's tracking set is the CLIENT's choice.
CLIENT_VIEW_DISTANCE = 6


def run(args):
    port = free_port()
    dest = os.path.join(BASE, "testserver-rc-%d" % port)
    if os.path.exists(dest):
        sys.exit("%s already exists; a reused directory is not a fresh one" % dest)

    os.makedirs(dest)
    game_dir = os.path.join(dest, "client-game")
    os.makedirs(game_dir)
    io.open(os.path.join(game_dir, "options.txt"), "wb").write(
        ("renderDistance:%d\n" % CLIENT_VIEW_DISTANCE).encode())
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
        # The client no longer injects a dirt stack into slot 0 on spawn
        # unless asked (it was item synthesis in the shipped build). No
        # witness needs the stack, but the baseline was calibrated with slot 0
        # holding dirt and selected, so keep that scene.
        env["REWO_GIVE_DIRT"] = "1"
        # One source of truth for both halves of r55/r56: the same literals
        # that went into server.properties are handed to the client, so the
        # witness compares what came off the WIRE against what the server was
        # configured with. Two hard-coded copies would drift; this cannot.
        env["REWO_RC_PACK_ID"] = PACK_ID.replace("-", "")
        env["REWO_RC_COC"] = COC_TEXT
        # M206 -- r73. The client's render distance comes from options.txt in
        # its game directory (vanilla's `buildPlayerInformation`), so stage
        # one, below the server's view-distance, and tell the witness the same
        # literal. The game directory is a scratch folder inside the server's
        # (removed with it), never the repo root, which was the client's game
        # directory before -- an options save during the run would have
        # written into the working tree.
        env["REWO_RC_VIEW_DISTANCE"] = str(CLIENT_VIEW_DISTANCE)
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
                 "--username", args.username,
                 "--game-dir", game_dir],
                cwd=ROOT,
                env=env,
                capture_output=True,
                timeout=float(os.environ.get("REWO_RC_TIMEOUT", "300")),
            )
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b"") + (e.stderr or b"")
            io.open(report_path(), "wb").write(out)
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
        report = report_path()
        io.open(report, "wb").write(text.encode("utf-8"))
        sys.stdout.buffer.write(text[-9000:].encode("utf-8", "replace"))
        # The tail above can cut off the failing witness (a CI log once showed
        # only "65/66"); name every non-PASS witness line explicitly.
        bad = [l for l in text.splitlines()
               if l.startswith("[rendercheck] ") and " PASS " not in l
               and not l.rstrip().endswith("witnesses")]
        if bad:
            sys.stdout.buffer.write(
                ("\nNOT PASSING:\n" + "\n".join(bad) + "\n").encode("utf-8", "replace"))
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
