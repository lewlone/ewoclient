"""M175's mutation battery - the isBaby whole-sheet swaps.

    python tools/m175_mutate.py [lo] [hi]

Checkers per mutation (M158's rule): `gate` = `rewo mobtexshot --check` (the
n-series baby witnesses + m8's baked/deferred split), `world` = the
`baby`-filtered rewo-world tests (the metadata routing into
EntityTable::set_baby). The live windowed path is not mutated here (needs a
server). reap() before every build; a timeout is a KILL; restore verified by
BYTES; the no-op control must SURVIVE.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
GPU = os.path.join(ROOT, "crates", "rewo-gpu", "src", "entities.rs")
LIVE = os.path.join(ROOT, "crates", "rewo-app", "src", "live_cmd.rs")
ASSETS = os.path.join(ROOT, "crates", "rewo-data", "src", "assets.rs")
TABLE = os.path.join(ROOT, "crates", "rewo-data", "src", "baby_texture_table.rs")
NETWORLD = os.path.join(ROOT, "crates", "rewo-net", "src", "lib.rs")

STRAYS = ("rewo.exe", "rewo_world-*.exe", "rewo_net-*.exe", "rewo_gpu-*.exe")

MUTATIONS = [
    ("control: no change", GPU,
     "pub(crate) struct Vertex {",
     "pub(crate) struct Vertex {",
     ["gate"], "MUST SURVIVE"),

    # ---- the draw --------------------------------------------------------
    ("is_baby never selects the swap at draw", GPU,
     ".filter(|_| d.mob.is_baby)",
     ".filter(|_| false)",
     ["gate"], "AbstractZombieRenderer.getTextureLocation returns the BABY location when state.isBaby - ignoring it renders a shrunken adult"),
    ("the offset applies to every slot", GPU,
     "                    let mut v = vec![[0.0f32, 0.0]; def.textures.len()];\n                    v[slot] = [",
     "                    let mut v = vec![[[0.4f32, 0.0]; def.textures.len()][0]; def.textures.len()];\n                    v[slot] = [",
     ["gate"], "only the ADULT sheet's slot moves - secondary layers keep their own sheets, so a blanket shift scrambles them"),

    # ---- the caller-side resolution -------------------------------------
    ("the kind lookup loses its namespace", LIVE,
     'let kind = rewo_gpu::mobs::kind_for_entity_name(&format!(\n                "minecraft:{}",\n                swap.entity\n            ));',
     'let kind = rewo_gpu::mobs::kind_for_entity_name(swap.entity);',
     ["gate"], "kind_for_entity_name strips a minecraft: prefix - bare names all fall through to Capsule and every swap goes inert (THE bug this milestone shipped with)"),

    # ---- the bake ---------------------------------------------------------
    ("no baby sheet is ever baked", ASSETS,
     "for swap in crate::baby_texture_table::BABY_SWAPS {\n        let adult_rel",
     "for swap in crate::baby_texture_table::BABY_SWAPS.iter().take(0) {\n        let adult_rel",
     ["gate"], "m8 pins baked == same-size table rows; baking none starves it"),

    # ---- the table ---------------------------------------------------------
    ("zombie's adult path points at husk", TABLE,
     'entity: "zombie",\n        baby_key: "zombie_baby",\n        baby_path: "textures/entity/zombie/zombie_baby.png",\n        adult_path: "textures/entity/zombie/zombie.png",',
     'entity: "zombie",\n        baby_key: "zombie_baby",\n        baby_path: "textures/entity/zombie/zombie_baby.png",\n        adult_path: "textures/entity/zombie/husk.png",',
     ["gate"], "the offset rides the slot whose key IS the adult sheet - pointing it at another mob's sheet resolves no slot and the swap goes inert for zombies only"),

    # ---- the wire ----------------------------------------------------------
    ("set_baby never fires from metadata", NETWORLD,
     "        } else {\n            entities.set_baby(eid, b);\n        }",
     "        } else {\n            let _ = b;\n        }",
     ["net", "world"], "the kind-split's fallthrough IS the baby path - dropping it starves EntityTable::babies and, one render later, the swap"),
]


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
    "net": lambda: unit("rewo-net", "baby_routing"),
    "gate": lambda: run([EXE, "mobtexshot", "--check"], 900),
    "world": lambda: unit("rewo-world", "baby"),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in (m[4] or [])}) or ["gate"]
    print(f"[m175] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control; checkers {used}")
    results = []
    for name, path, find, repl, checkers, why in selected:
        if find is None:
            print(f"SKIP      {name}")
            continue
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
                if is_control:
                    verdict, reason = "FAILED", "control build failed"
                else:
                    verdict, reason = "KILLED", "build failed"
            else:
                for c in checkers:
                    code, r = CHECKERS[c]()
                    if code is None:
                        verdict, reason = "TIMEOUT", c
                        break
                    if code != 0:
                        last = r.strip().splitlines()[-1][:120] if r.strip() else "no output"
                        reason = f"killed by {c} ({last})"
                        break
                else:
                    reason = "all checkers green"
            if verdict is None:
                killed = reason.startswith("killed")
                if is_control:
                    verdict = "SURVIVED" if not killed else "FAILED"
                else:
                    verdict = "KILLED" if killed else "SURVIVED"
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
    bad = [n for n, v, _ in results if v in ("SURVIVED", "FAILED", "TIMEOUT")]
    ctrl_ok = any(n.startswith("control") and v == "SURVIVED" for n, v, _ in results)
    print(f"[m175] {killed} killed, control {'ok' if ctrl_ok else 'FAILED'}, survivors: {bad}")


if __name__ == "__main__":
    main()
