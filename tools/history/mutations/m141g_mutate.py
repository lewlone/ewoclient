"""M141g's mutation battery — the three state indices, the guardian, the sniffer.

    python tools/m141g_mutate.py

Same rules as its predecessors: verdicts from the TEST RESULT LINE rather than
the exit code, a NO-OP CONTROL that must SURVIVE, restore in a `finally`, a
per-run timeout, and a byte comparison at the end.

**Run it alone.** Three batteries in one command hit the ten-minute tool cap
during M141f, m141e's `finally` never ran and it left a mutation on disk. If a
battery is ever interrupted, check every battery's ORIGINAL strings before
anything else — `git status` cannot tell a leftover mutation from uncommitted
work.
"""
import io
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
M = os.path.join("crates", "rewo-net", "src", "metadata.rs")
L = os.path.join("crates", "rewo-net", "src", "lib.rs")
E = os.path.join("crates", "rewo-net", "src", "sound_engine.rs")
W = os.path.join("crates", "rewo-world", "src", "entities.rs")
P = os.path.join("crates", "rewo-net", "src", "play.rs")

MUTATIONS = [
    (
        W,
        "CONTROL: a comment-only edit (must SURVIVE)",
        "/// `Sniffer.State.SEARCHING` — ordinal 4.",
        "/// `Sniffer.State.SEARCHING` is ordinal 4.",
        "SURVIVED",
    ),
    # --- the three indices (M141g1's live bug) ----------------------------
    (
        M,
        "the sniffer state goes back to index 17",
        "            (18, 35) => meta.gesture_state = r.varint().ok().map(|v| v as u8), // SNIFFER_STATE",
        "            (17, 35) => meta.gesture_state = r.varint().ok().map(|v| v as u8), // SNIFFER_STATE",
        "KILLED",
    ),
    (
        M,
        "the armadillo state goes back to index 17",
        "            (18, 36) => meta.gesture_state = r.varint().ok().map(|v| v as u8), // ARMADILLO_STATE",
        "            (17, 36) => meta.gesture_state = r.varint().ok().map(|v| v as u8), // ARMADILLO_STATE",
        "KILLED",
    ),
    (
        M,
        "the copper golem is moved off 17 with the other two",
        "            (17, 37) => meta.gesture_state = r.varint().ok().map(|v| v as u8), // COPPER_GOLEM_STATE",
        "            (18, 37) => meta.gesture_state = r.varint().ok().map(|v| v as u8), // COPPER_GOLEM_STATE",
        "KILLED",
    ),
    # --- the guardian's target and counter --------------------------------
    (
        L,
        "the attack target is stored for any entity, not just a guardian",
        "        if Some(type_id) == kinds.guardian || Some(type_id) == kinds.elder_guardian {\n            entities.set_guardian_attack_target(eid, target);\n        }",
        "        entities.set_guardian_attack_target(eid, target);",
        "KILLED",
    ),
    (
        L,
        "an elder guardian's target is ignored",
        "        if Some(type_id) == kinds.guardian || Some(type_id) == kinds.elder_guardian {",
        "        if Some(type_id) == kinds.guardian {",
        "KILLED",
    ),
    (
        W,
        "the counter runs without a target",
        "            if g.has_target() && g.client_side_attack_time < GUARDIAN_MAX_ATTACK_DURATION {",
        "            if g.client_side_attack_time < GUARDIAN_MAX_ATTACK_DURATION {",
        "KILLED",
    ),
    (
        W,
        "the counter is uncapped",
        "            if g.has_target() && g.client_side_attack_time < GUARDIAN_MAX_ATTACK_DURATION {",
        "            if g.has_target() {",
        "KILLED",
    ),
    (
        W,
        "a target arriving does not reset the counter",
        "                target,\n                client_side_attack_time: 0,",
        "                target,\n                client_side_attack_time: self\n                    .guardian_attack\n                    .get(&id)\n                    .map_or(0, |g| g.client_side_attack_time),",
        "KILLED",
    ),
    (
        W,
        # Retargeted: this predicate had **no caller at all** when the battery
        # first ran, so mutating it changed nothing and it reported as a
        # survivor. That is what a dead accessor looks like from outside. The
        # tick calls it now, so there is one definition and two callers.
        "hasActiveAttackTarget treats 0 as a real entity",
        "    fn has_target(&self) -> bool {\n        self.target != 0\n    }",
        "    fn has_target(&self) -> bool {\n        true\n    }",
        "KILLED",
    ),
    # --- the sniffer's predicate ------------------------------------------
    (
        W,
        "the sniffer sound needs DIGGING and not SEARCHING",
        "        matches!(self.gesture_state(id), SNIFFER_SEARCHING | SNIFFER_DIGGING)",
        "        self.gesture_state(id) == SNIFFER_DIGGING",
        "KILLED",
    ),
    (
        W,
        "the sniffer state ordinals are off by one",
        "pub const SNIFFER_SEARCHING: u8 = 4;\n/// `Sniffer.State.DIGGING` — ordinal 5.\npub const SNIFFER_DIGGING: u8 = 5;",
        "pub const SNIFFER_SEARCHING: u8 = 3;\n/// `Sniffer.State.DIGGING` — ordinal 5.\npub const SNIFFER_DIGGING: u8 = 4;",
        "KILLED",
    ),
    # --- the instances ----------------------------------------------------
    (
        E,
        "the guardian's beam is attenuated",
        "                attenuation: Attenuation::None,\n                x,\n                y,\n                z,\n                binding: Binding::Entity(guardian),",
        "                x,\n                y,\n                z,\n                binding: Binding::Entity(guardian),",
        "KILLED",
    ),
    (
        E,
        "the sniffer's one-shot is made a loop",
        "                looping: false,\n                delay: 0,\n                x,\n                y,\n                z,\n                binding: Binding::Entity(sniffer),",
        "                looping: true,\n                delay: 0,\n                x,\n                y,\n                z,\n                binding: Binding::Entity(sniffer),",
        "KILLED",
    ),
    (
        E,
        "the guardian's scale uses the elder's duration",
        "            rewo_world::entities::GUARDIAN_MAX_ATTACK_DURATION,",
        "            60,",
        "KILLED",
    ),
    # --- the trigger, a composition root ----------------------------------
    (
        P,
        "COMPOSITION ROOT: the event trigger is never called (must SURVIVE)",
        "            self.entity_event_sound(body);",
        "",
        "SURVIVED",
    ),
    (
        P,
        "COMPOSITION ROOT: the two event ids are swapped (must SURVIVE)",
        "            21 if is_guardian => crate::sounds::TickableSound::GuardianAttack { guardian: eid },",
        "            63 if is_guardian => crate::sounds::TickableSound::GuardianAttack { guardian: eid },",
        "SURVIVED",
    ),
]


def run_tests():
    """Returns "ok", "failed" or "build" — see `m141_mutate.py`'s note."""
    for attempt in range(2):
        outs, rcs = [], []
        for args in (
            ["cargo", "test", "-p", "rewo-world", "--lib"],
            ["cargo", "test", "-p", "rewo-net", "--lib"],
        ):
            try:
                p = subprocess.run(args, cwd=ROOT, capture_output=True, timeout=300)
            except subprocess.TimeoutExpired:
                for exe in ("rewo_world-*.exe", "rewo_net-*.exe"):
                    subprocess.run(["taskkill", "/F", "/IM", exe], capture_output=True)
                return "failed"
            outs.append((p.stdout + p.stderr).decode("utf-8", "replace"))
            rcs.append(p.returncode)
        joined = "\n".join(outs)
        if "test result: FAILED" in joined:
            return "failed"
        if all("test result: ok" in o for o in outs) and all(r == 0 for r in rcs):
            return "ok"
        if attempt == 0:
            time.sleep(3)
            continue
        sys.stderr.write(joined[-2000:] + "\n")
        return "build"
    return "build"


def main():
    paths = sorted({m[0] for m in MUTATIONS})
    snapshots = {p: io.open(os.path.join(ROOT, p), "rb").read() for p in paths}

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != "ok":
        sys.exit("BASELINE FAILS -- every verdict below would be meaningless")
    print("pass")

    bad = 0
    for rel, name, old, new, want in MUTATIONS:
        path = os.path.join(ROOT, rel)
        snapshot = snapshots[rel]
        text = snapshot.decode("utf-8")
        n = text.count(old)
        if n != 1:
            print("%-60s ANCHOR MATCHED %d TIMES" % (name[:60], n))
            bad += 1
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            r = run_tests()
            verdict = {"failed": "KILLED", "ok": "SURVIVED", "build": "BUILD-FAIL"}[r]
        finally:
            io.open(path, "wb").write(snapshot)
        ok = verdict == want
        bad += 0 if ok else 1
        print(
            "%-60s %-10s (want %-9s) %s"
            % (name[:60], verdict, want, "ok" if ok else "<<< UNEXPECTED")
        )

    leftover = [
        p for p in paths if io.open(os.path.join(ROOT, p), "rb").read() != snapshots[p]
    ]
    print("-----")
    print(
        "files restored: %s"
        % ("no -- MUTATION LEFT ON DISK: %s" % leftover if leftover else "yes")
    )
    print("%d/%d as expected" % (len(MUTATIONS) - bad, len(MUTATIONS)))
    sys.exit(1 if (bad or leftover) else 0)


if __name__ == "__main__":
    main()
