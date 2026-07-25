"""Machine-extract, from the decompiled 26.2 jar, which registered entity types
are `LivingEntity`s and which of them run `LivingEntity.updateSwingTime()` on
the **client** — into `crates/rewo-data/src/entity_classes.rs`.

Why this exists
---------------
M19's combat swing needs two sets that no datagen report carries:

1. **Living.** `ClientPacketListener` gates the swing inputs on the entity's
   Java type: `handleAnimate` casts to `LivingEntity`, `handleSetEquipment`
   tests `instanceof LivingEntity`, `handleUpdateMobEffect` likewise. A packet
   naming a boat or an arrow must mutate nothing.

2. **Swing-ticking.** `updateSwingTime` is NOT called from `LivingEntity.tick`.
   On the client it is called from exactly four places (asserted below):
   `Player.aiStep`, `RemotePlayer.tick`, `Monster.aiStep` and `Mannequin.tick`.
   So a cow or a hoglin can be sent `swing()` — `Mob.doHurtTarget` does that —
   and its `attackAnim` still never advances. Getting this wrong is invisible
   in a render (the pose is simply frozen at 0) but wrong on the wire model,
   and OptiFine CEM's `swing_progress` reads it for *every* mob, not just the
   humanoid player.

Extraction, all from the decompile (REWO_PLAN §11), never a wiki:

* `EntityTypes.java` — `public static final EntityType<Cls> NAME = register(
  EntityTypeIds.NAME, …)` gives registry-constant → implementation class.
* `EntityTypeIds.java` — `NAME = create("cow")` gives the registry string.
* Every `net/minecraft/**/*.java` class declaration gives `class X extends Y`,
  which is walked recursively to the root.

Re-run after a version bump:

    python tools/gen_entity_classes.py

Fails loud rather than defaulting: an unparsed registration, an unresolved id
constant, an `extends` chain that never reaches `Entity`, a duplicate class
name, or a change in the set of `updateSwingTime()` call sites are all hard
errors. In particular, if 26.3 adds a fifth caller the generator stops here
instead of silently shipping a set that is one class short.
"""
import os
import re
import sys

VERSION = "26.2"
DECOMP = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", VERSION, "decompiled")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "entity_classes.rs")

ENTITY_TYPES = os.path.join(DECOMP, "net/minecraft/world/entity/EntityTypes.java")
ENTITY_TYPE_IDS = os.path.join(DECOMP, "net/minecraft/world/entity/EntityTypeIds.java")

# `public static final EntityType<Cow> COW = register(` … `EntityTypeIds.COW,`
REGISTRATION = re.compile(
    r'public static final EntityType<([A-Za-z0-9_.]+)>\s+([A-Z0-9_]+)\s*=\s*register\(\s*'
    r'EntityTypeIds\.([A-Z0-9_]+)\s*,',
    re.S)
# `public static final ResourceKey<EntityType<?>> COW = create("cow");`
ID_CONST = re.compile(
    r'ResourceKey<EntityType<\?>>\s+([A-Z0-9_]+)\s*=\s*create\(\s*"([a-z0-9_/]+)"\s*\)')
# `public abstract class Zombie extends Monster implements …`
# (indented, because Vineflower writes nested entity classes inline —
# `Display.BlockDisplay` lives in `Display.java` as `public static class …`)
CLASS_DECL = re.compile(
    r'^[ \t]*(?:public\s+|private\s+|protected\s+|static\s+|final\s+|abstract\s+'
    r'|sealed\s+|non-sealed\s+)*class\s+'
    r'([A-Za-z0-9_]+)(?:<[^{]*?>)?\s+extends\s+([A-Za-z0-9_.]+)',
    re.M)

# The client call sites this table is derived from. A change here changes the
# answer, so the generator asserts the set rather than trusting it.
EXPECTED_CALLERS = {
    "net/minecraft/client/player/RemotePlayer.java",
    "net/minecraft/world/entity/decoration/Mannequin.java",
    "net/minecraft/world/entity/monster/Monster.java",
    "net/minecraft/world/entity/player/Player.java",
}
# The declaring classes of those call sites. `RemotePlayer` is a `Player`
# descendant, so the roots that matter are these three.
SWING_TICK_ROOTS = ("Monster", "Player", "Mannequin")
LIVING_ROOT = "LivingEntity"
ROOT = "Entity"

# M20: sets derived purely from the `extends` chain, for the metadata slots
# whose meaning depends on which class introduced the accessor. Each entry is
# (rust const, java ancestor, doc lines). Counting `defineId` up the hierarchy
# gives the slot: Entity 0..7, LivingEntity 8..14, Mob 15, PathfinderMob /
# Monster / PatrollingMonster add none, Raider 16, then SpellcasterIllager 17
# and Pillager 17 in parallel branches.
ANCESTRY_SETS = [
    ("MOB", "Mob", [
        "Registry names whose class descends from `Mob`, which introduces",
        "`DATA_MOB_FLAGS_ID` at **index 15, BYTE** (bit 1 no-AI, bit 2",
        "left-handed, bit 4 aggressive). `ArmorStand.DATA_CLIENT_FLAGS` is also",
        "a BYTE at 15 and means something else entirely, so the flags byte may",
        "only be read for a type in this set.",
    ]),
    ("RAIDER", "Raider", [
        "Registry names whose class descends from `Raider`, which introduces",
        "`IS_CELEBRATING` at **index 16 with the BOOLEAN serializer** — the same",
        "slot and serializer as `AgeableMob`/`Zombie.DATA_BABY_ID` and",
        "`Allay.DATA_DANCING`. The byte parser cannot separate them; only the",
        "entity kind can, which is why this set exists.",
    ]),
    ("SPELLCASTER_ILLAGER", "SpellcasterIllager", [
        "Registry names whose class descends from `SpellcasterIllager`, which",
        "introduces `DATA_SPELL_CASTING_ID` at **index 17, BYTE**. Non-zero means",
        "`isCastingSpell()`, which selects the SPELLCASTING arm pose.",
    ]),
    ("ILLAGER", "AbstractIllager", [
        "Registry names whose class descends from `AbstractIllager` — the models",
        "that run `IllagerModel.setupAnim`'s arm-pose switch rather than the",
        "humanoid/undead rigs.",
    ]),
]


def die(msg):
    print(f"gen_entity_classes: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        return f.read()


def scan_class_graph():
    """-> ({class: superclass}, {class: {supers}}) over the whole decompile.

    Two tiers, because Vineflower writes nested entity implementations inline:
    `Display.BlockDisplay` lives in `Display.java`. The file's own top-level
    class is authoritative (its name is unique across the tree); a nested class
    only fills a gap, and only when every declaration of that simple name
    agrees on a superclass. Anything ambiguous stays out and [`chain`] fails
    loud if it is ever needed.
    """
    top, where, nested = {}, {}, {}
    for root, _dirs, files in os.walk(os.path.join(DECOMP, "net")):
        for fname in files:
            if not fname.endswith(".java"):
                continue
            path = os.path.join(root, fname)
            own = fname[: -len(".java")]
            for cls, sup in CLASS_DECL.findall(read(path)):
                sup = sup.split(".")[-1]
                if cls == own:
                    if cls in top and top[cls] != sup:
                        die(f"top-level class {cls} declared twice with different "
                            f"supers ({top[cls]} vs {sup}) — {where[cls]} / {path}")
                    top[cls] = sup
                    where[cls] = path
                else:
                    nested.setdefault(cls, set()).add(sup)
    return top, nested


def chain(cls, top, nested):
    """Every class from `cls` up to (and including) the root, or None."""
    out, seen = [], set()
    cur = cls
    while cur and cur not in seen:
        seen.add(cur)
        out.append(cur)
        if cur == ROOT:
            return out
        if cur in top:
            cur = top[cur]
        else:
            cands = nested.get(cur)
            if not cands:
                return None
            if len(cands) > 1:
                die(f"nested class {cur} has ambiguous supers {sorted(cands)} — "
                    "cannot resolve its hierarchy")
            cur = next(iter(cands))
    return None


def main():
    for p in (ENTITY_TYPES, ENTITY_TYPE_IDS):
        if not os.path.isfile(p):
            die(f"missing {p}")

    # 1. The four client call sites must be exactly what this table assumes.
    callers = set()
    for root, _dirs, files in os.walk(os.path.join(DECOMP, "net")):
        for fname in files:
            if not fname.endswith(".java"):
                continue
            path = os.path.join(root, fname)
            if "this.updateSwingTime()" in read(path):
                callers.add(os.path.relpath(path, DECOMP).replace("\\", "/"))
    if callers != EXPECTED_CALLERS:
        die("the set of updateSwingTime() call sites changed:\n"
            f"  expected {sorted(EXPECTED_CALLERS)}\n"
            f"  found    {sorted(callers)}\n"
            "Update SWING_TICK_ROOTS + EXPECTED_CALLERS together, deliberately.")

    # 2. registry name -> implementation class.
    ids = dict(ID_CONST.findall(read(ENTITY_TYPE_IDS)))
    src = read(ENTITY_TYPES)
    regs = REGISTRATION.findall(src)
    total_register = len(re.findall(r"=\s*register\(", src))
    if len(regs) != total_register:
        die(f"parsed {len(regs)} of {total_register} `register(` calls in "
            "EntityTypes.java — a registration shape changed")
    by_name = {}
    for cls, field, id_const in regs:
        if field != id_const:
            die(f"registration {field} names EntityTypeIds.{id_const} — "
                "the constant/field pairing is no longer 1:1")
        name = ids.get(id_const)
        if name is None:
            die(f"EntityTypeIds.{id_const} has no create(\"…\") literal")
        by_name["minecraft:" + name] = cls.split(".")[-1]

    # 3. Walk each implementation class to the root.
    top, nested = scan_class_graph()
    living, ticking = [], []
    ancestry = {key: [] for key, _root, _doc in ANCESTRY_SETS}
    for name, cls in sorted(by_name.items()):
        path = chain(cls, top, nested)
        if path is None:
            die(f"{name}: class {cls} has no `extends` chain reaching {ROOT}")
        if LIVING_ROOT in path:
            living.append(name)
        if any(r in path for r in SWING_TICK_ROOTS):
            if LIVING_ROOT not in path:
                die(f"{name}: {cls} ticks a swing but is not a {LIVING_ROOT}")
            ticking.append(name)
        for key, root, _doc in ANCESTRY_SETS:
            if root in path:
                ancestry[key].append(name)
    # M20's metadata routing keys on these, so an empty one means the class
    # was renamed and every pose derived from it would silently go neutral.
    for key, root, _doc in ANCESTRY_SETS:
        if not ancestry[key]:
            die(f"no registered type descends from {root} — {key} would be "
                "empty and its metadata slot would route to nothing")

    with open(OUT, "w", encoding="utf-8", newline="\n") as out:
        out.write("//! GENERATED by `tools/gen_entity_classes.py` — do not edit.\n//!\n")
        out.write(f"//! Source: the decompiled {VERSION} client\n")
        out.write("//! (`EntityTypes.java` + `EntityTypeIds.java` + every `class X extends Y`\n")
        out.write("//! declaration under `net/`). Re-run after a version bump; the script\n")
        out.write("//! fails loud on an unparsed registration, an unresolved id constant, a\n")
        out.write("//! broken `extends` chain, or a change in the `updateSwingTime()` call\n")
        out.write("//! sites this table is derived from.\n//!\n")
        out.write(f"//! {len(by_name)} registered types, {len(living)} living, "
                  f"{len(ticking)} swing-ticking.\n\n")
        out.write("/// Registered entity types in this version — a runtime registry of a\n")
        out.write("/// different size means the pin has drifted.\n")
        out.write(f"pub const REGISTERED_TYPES: usize = {len(by_name)};\n\n")
        out.write("/// The client call sites `SWING_TICKING` was derived from, recorded so a\n")
        out.write("/// reader can re-check the claim without re-running the generator.\n")
        out.write("pub const SWING_TICK_CALL_SITES: &[&str] = &[\n")
        for c in sorted(EXPECTED_CALLERS):
            out.write(f'    "{c}",\n')
        out.write("];\n\n")
        out.write("/// Registry names whose implementation class descends from\n")
        out.write("/// `LivingEntity`. `handleAnimate` casts to it, `handleSetEquipment` and\n")
        out.write("/// `handleUpdateMobEffect` test `instanceof` it — anything else is inert.\n")
        out.write("pub const LIVING: &[&str] = &[\n")
        for n in living:
            out.write(f'    "{n}",\n')
        out.write("];\n\n")
        out.write("/// Registry names whose **client** class runs\n")
        out.write("/// `LivingEntity.updateSwingTime()` every tick — a `Monster`, `Player` or\n")
        out.write("/// `Mannequin` descendant. Every other living entity can accept a swing\n")
        out.write("/// and never advance it, exactly as vanilla does.\n")
        out.write("pub const SWING_TICKING: &[&str] = &[\n")
        for n in ticking:
            out.write(f'    "{n}",\n')
        out.write("];\n")
        for key, root, doc in ANCESTRY_SETS:
            out.write("\n")
            for line in doc:
                out.write(f"/// {line}\n" if line else "///\n")
            out.write(f"pub const {key}: &[&str] = &[\n")
            for n in ancestry[key]:
                out.write(f'    "{n}",\n')
            out.write("];\n")
    print(f"gen_entity_classes: {len(by_name)} types, {len(living)} living, "
          f"{len(ticking)} swing-ticking -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
