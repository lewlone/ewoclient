"""Mutation battery for the real-texture mob gate (`rewo mobtexshot --check`)
and the dynamic-pool slot ring.

Run one batch at a time; each gate-routed mutation costs a release-free
incremental rebuild plus a full gate run, and a battery that overruns the
10-minute tool cap has its `finally` skipped and **leaves the mutation on
disk** (REWO_PLAN.md §0.0 gotcha -1a). Batches are named so a run is one
argument:

    python tools/mobtex_mutate.py gate     # du + skin_uv sensitivity
    python tools/mobtex_mutate.py gate2    # attribution, vacuity, emissive
    python tools/mobtex_mutate.py gate3    # the pinned counts and the fail-open paths
    python tools/mobtex_mutate.py pools    # the two CALL SITES of the slot ring
    python tools/mobtex_mutate.py ring     # the slot ring's unit tests

Every batch opens with a BASELINE that must PASS and carries a NO-OP CONTROL
that must SURVIVE. Without both, a battery run against an already-red command
reads KILLED for every entry and proves nothing — which is how M109 lost two
whole batteries (AGENT_LOOP_BRIEF).

Verdicts come from EXIT CODES, never from a substring: `mobtexshot` is
fail-closed on a declared witness count, so a run can print `ok` on every line
and still be red.

**`ring` and `pools` are not the same claim, and the difference is the whole
reason `pools` exists.** `ring` mutates `SlotRing::claim` and is graded by its
unit tests; `pools` mutates the two CALLERS, which is where the bug lived. An
adversarial review deleted both `remove(&old)` blocks — restoring the exact
aliasing bug — and `mobtexshot` stayed 10/10, `rewo-gpu` 293/293, `itemshot` and
`mobshot` green, because nothing anywhere graded a call site. A battery that
mutates only the helper reports 3/3 against a feature that could be deleted
whole.

One mutation below is deliberately recorded as an EXPECTED SURVIVOR rather than
omitted: see `first_unexplained`'s empty-set arm in `gate3`.
"""

import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ENTITIES = os.path.join(ROOT, "crates", "rewo-gpu", "src", "entities.rs")
GATE = os.path.join(ROOT, "crates", "rewo-app", "src", "mobtexshot_cmd.rs")
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")

# Per-run timeout. A hung mutant that takes the battery down with it is the
# other half of gotcha -1a; a timeout makes it a KILL instead of an outage.
TIMEOUT = 600


def read(path):
    with open(path, "rb") as f:
        return f.read()


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)
    # `mv`/`cp` preserve the ORIGINAL mtime, which is older than the mutated
    # build, so cargo skips the rebuild and the next run silently grades the
    # previous mutant (REWO_PLAN §0.0 gotcha 0b). Touch forward explicitly.
    now = time.time() + 1
    os.utime(path, (now, now))


def build():
    r = subprocess.run(
        ["cargo", "build", "-p", "rewo-app", "--message-format", "short"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=TIMEOUT,
    )
    return r.returncode == 0, r.stdout + r.stderr


def run_gate():
    r = subprocess.run(
        [EXE, "mobtexshot", "--check"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=TIMEOUT,
    )
    return r.returncode


def run_tests(filt):
    r = subprocess.run(
        ["cargo", "test", "-p", "rewo-gpu", "--lib", filt],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=TIMEOUT,
    )
    # A crate whose tests fail to COMPILE prints no `test result` line at all
    # and would otherwise read as a pass (AGENT_LOOP_BRIEF).
    out = r.stdout + r.stderr
    if "test result:" not in out:
        return None, out
    return r.returncode == 0, out


# (name, file, find, replace, runner, expect)
#   expect "KILL"    — the check must go RED
#   expect "SURVIVE" — the check must stay GREEN (the control)
NOOP = (
    "noop-control: reword the summary line",
    GATE,
    b'println!("[mobtexshot] {pass}/{} witnesses", w.len());',
    b'println!("[mobtexshot] {pass} of {} witnesses", w.len());',
    "gate",
    "SURVIVE",
)

GATE_BATCH = [
    NOOP,
    (
        "du forced to the measured zombie->villager delta (-0.25 U)",
        ENTITIES,
        b"let du = variant_uv.map_or(skin_du, |v| v[q.tex as usize]);",
        b"let du = { let _ = variant_uv; [-0.25f32, 0.0f32] };",
        "gate",
        "KILL",
    ),
    (
        "skin_uv applied unconditionally again (the pre-fix pass)",
        ENTITIES,
        b"""        let skin_du = match d.kind {
            EntityModelKind::Player | EntityModelKind::PlayerSlim => {
                d.skin_uv.unwrap_or([0.0, 0.0])
            }
            _ => [0.0, 0.0],
        };""",
        b"        let skin_du = d.skin_uv.unwrap_or([0.0, 0.0]);",
        "gate",
        "KILL",
    ),
    (
        "skin_uv ignored for players too (the over-fix)",
        ENTITIES,
        b"""        let skin_du = match d.kind {
            EntityModelKind::Player | EntityModelKind::PlayerSlim => {
                d.skin_uv.unwrap_or([0.0, 0.0])
            }
            _ => [0.0, 0.0],
        };""",
        b"        let skin_du = [0.0f32, 0.0f32];",
        "gate",
        "KILL",
    ),
]

GATE2_BATCH = [
    NOOP,
    (
        "leave-one-out attribution returns every pixel",
        GATE,
        b"        if full[i..i + 3] != without[i..i + 3] {\n            out.push(pack(full[i], full[i + 1], full[i + 2]));\n        }",
        b"        let _ = &without[i..i + 3];\n        out.push(pack(full[i], full[i + 1], full[i + 2]));",
        "gate",
        "KILL",
    ),
    (
        "a mob's own set becomes EVERY sheet (the vacuity probe)",
        GATE,
        b"    let sets: Vec<&HashSet<u32>> = own.keys.iter().filter_map(|k| sheets.get(k)).collect();",
        b"    let sets: Vec<&HashSet<u32>> = sheets.values().collect();\n    let _ = &own.keys;",
        "gate",
        "KILL",
    ),
    (
        "the emissive layers drop out of a mob's own set entirely",
        GATE,
        b"""            for l in rewo_gpu::mobs::emissive_layers(d.kind) {
                if !keys.contains(&l.tex) {
                    keys.push(l.tex);
                }""",
        b"""            for l in rewo_gpu::mobs::emissive_layers(d.kind).iter().take(0) {
                if !keys.contains(&l.tex) {
                    keys.push(l.tex);
                }""",
        "gate",
        "KILL",
    ),
]

GATE3_BATCH = [
    NOOP,
    (
        # m8's name says "exactly this size" and its first version asserted
        # `jar_babies > 0`, so 147 lived only in the detail string. Changing
        # what the jar scan counts must now turn it red.
        "the jar's baby-sheet count changes (the pinned 147 moves)",
        GATE,
        b'            && name.contains("baby")\n        {',
        b'            && name.contains("baby")\n            && !name.contains("zombie")\n        {',
        "gate",
        "KILL",
    ),
    (
        # `first_unexplained` used to answer `None` — "every pixel explained" —
        # when a kind's declared sheets did not resolve, i.e. it was greenest
        # exactly where it knew least. Unreachable with the real registry, so
        # the mutation makes it reachable.
        #
        # **ONE kind, not all of them, and the difference is the point.**
        # Blanking every kind's sheets is killed either way, because `m9` feeds
        # the villager's pixels to the ZOMBIE's set and a blank zombie set
        # makes it answer `None` — so that version proves the negative control
        # works and says nothing about the arm it names. Blanking the villager
        # alone leaves `m9` intact: fail-closed this is red (m1/m5/m12 report
        # the villager's pixels as unexplained), fail-open it was green.
        "ONE kind's declared sheets stop resolving (isolates the empty-set arm)",
        GATE,
        b"            let mut keys = d.textures.to_vec();",
        b"            let mut keys = if d.kind == rewo_gpu::mobs::EntityModelKind::Villager {\n"
        b'                vec!["rewo:no-such-sheet"]\n'
        b"            } else {\n"
        b"                d.textures.to_vec()\n"
        b"            };",
        "gate",
        "KILL",
    ),
    (
        # "Drew nothing" used to pass as a printed SKIP: `graded >= 60` against
        # a measured 81 tolerated 21 kinds vanishing. One kind attributed no
        # pixels must now break the accounting identity and the pinned SKIP
        # bucket together.
        "one kind in the sweep draws nothing (the SKIP bucket grows to 10)",
        GATE,
        b"        let wo = big.shoot(gpu, &cast, Some(i))?;\n        let px = attributed(&full, &wo);",
        b"        let wo = big.shoot(gpu, &cast, Some(i))?;\n        let px = if i == 0 { Vec::new() } else { attributed(&full, &wo) };",
        "gate",
        "KILL",
    ),
]

# The mutations the reviewer applied, verbatim. Each is one call site, so they
# are separate entries: covering only one of them would leave the other
# deletable, and that is the failure mode this batch exists to prove is gone.
POOLS_BATCH = [
    NOOP,
    (
        "the ITEM pool's caller keeps the key it just evicted",
        ENTITIES,
        b"                let (slot, evicted) = self.item_ring.claim(q.tex);\n"
        b"                if let Some(old) = evicted {\n"
        b"                    self.item_slots.remove(&old);\n"
        b"                }\n",
        b"                let (slot, _evicted) = self.item_ring.claim(q.tex);\n",
        "gate",
        "KILL",
    ),
    (
        "the TRIM pool's caller keeps the key it just evicted",
        ENTITIES,
        b"        let (slot, evicted) = self.trim_ring.claim(key.to_string());\n"
        b"        if let Some(old) = evicted {\n"
        b"            self.trim_slots.remove(&old);\n"
        b"        }\n",
        b"        let (slot, _evicted) = self.trim_ring.claim(key.to_string());\n",
        "gate",
        "KILL",
    ),
    (
        # The pools' witnesses must not be satisfied by a pool that never
        # wrapped: if the over-fill stopped one short, `m10`/`m11` would be
        # asserting a full-but-unwrapped pool, which the pre-fix code also has.
        "the trim over-fill stops one short (the wrap never happens)",
        GATE,
        b"    for i in 0..TRIM_POOL {",
        b"    for i in 0..TRIM_POOL - 1 {",
        "gate",
        "KILL",
    ),
]

RING_BATCH = [
    (
        "noop-control: reword a SlotRing doc line",
        ENTITIES,
        b"/// A round-robin atlas-slot ring that says **whose slot it just took**.",
        b"/// A round-robin atlas-slot ring naming whose slot it just took.",
        "ring",
        "SURVIVE",
    ),
    (
        "claim never names the key it evicted",
        ENTITIES,
        b"        let evicted = self.owner[slot as usize].take();",
        b"        let evicted = { self.owner[slot as usize] = None; None };",
        "ring",
        "KILL",
    ),
    (
        "the cursor is not taken modulo the pool size",
        ENTITIES,
        b"        let slot = self.next % self.cap;\n        self.next = self.next.wrapping_add(1);",
        b"        let slot = self.next.min(self.cap - 1);\n        self.next = self.next.wrapping_add(1);",
        "ring",
        "KILL",
    ),
    (
        "the ring writes the new owner but forgets it",
        ENTITIES,
        b"        self.owner[slot as usize] = Some(key);",
        b"        let _ = key;",
        "ring",
        "KILL",
    ),
]


def main():
    which = sys.argv[1] if len(sys.argv) > 1 else "gate"
    batch = {
        "gate": GATE_BATCH,
        "gate2": GATE2_BATCH,
        "gate3": GATE3_BATCH,
        "pools": POOLS_BATCH,
        "ring": RING_BATCH,
    }[which]

    # BASELINE. Everything below is meaningless if the tree is already red.
    ok, log = build()
    if not ok:
        print("BASELINE BUILD FAILED\n" + log[-2000:])
        return 1
    # The baseline must run whatever the batch's ENTRIES run, or a "gate2" run
    # would open by proving the ring's unit tests green and then grade a gate
    # it never checked.
    runners = {r for (_, _, _, _, r, _) in batch}
    base = True
    if "gate" in runners:
        base = base and run_gate() == 0
    if "ring" in runners:
        g, _ = run_tests("slot_ring")
        base = base and g is True
    print(f"baseline: {'GREEN' if base else 'RED'}")
    if not base:
        print("ABORT — a battery against an already-red check scores every")
        print("mutation KILLED and proves nothing.")
        return 1

    results = []
    for name, path, find, repl, runner, expect in batch:
        orig = read(path)
        n = orig.count(find)
        if n != 1:
            results.append((name, expect, f"ANCHOR x{n} — SKIPPED"))
            print(f"  ! {name}: anchor matched {n} times, skipped")
            continue
        try:
            write(path, orig.replace(find, repl))
            built, log = build()
            if not built:
                verdict = "BUILD-FAIL"
            else:
                if runner == "gate":
                    green = run_gate() == 0
                else:
                    green, _ = run_tests("slot_ring")
                    if green is None:
                        green = False
                verdict = "SURVIVED" if green else "KILLED"
        finally:
            write(path, orig)
        ok = (verdict == "KILLED") if expect == "KILL" else (verdict == "SURVIVED")
        results.append((name, expect, verdict + ("" if ok else "  <-- UNEXPECTED")))
        print(f"  {'ok ' if ok else 'BAD'} {name}: {verdict} (wanted {expect})")

    # The restore does not rebuild, so the next thing to run would grade the
    # last mutant's binary (REWO_PLAN §0.0 gotcha 0d). Rebuild before exiting.
    build()

    print("\n--- summary ---")
    for name, expect, verdict in results:
        print(f"{expect:8} {verdict:24} {name}")
    bad = [r for r in results if "UNEXPECTED" in r[2] or "SKIPPED" in r[2]]
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
