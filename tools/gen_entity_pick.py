"""Machine-extract, from the decompiled 26.2 jar, the two per-entity-type facts
the crosshair entity pick needs — into `crates/rewo-data/src/entity_pick_table.rs`.

Why this exists
---------------
M73's entity raycast sweeps every candidate's bounding box, and two of its
inputs are Java facts no datagen report carries:

1. **Base dimensions.** `Entity.getBoundingBox()` is
   `EntityDimensions.makeBoundingBox(pos)` = `AABB(x-w/2, y, z-w/2, x+w/2,
   y+h, z+w/2)`, and `w`/`h` come from `EntityType.Builder.sized(w, h)`. Before
   M73 `EntityTypes::dimensions` was a hand-written 14-entry table with a
   humanoid default for everything else, which is fine for a debug capsule and
   not fine for "did the ray hit this mob".

2. **`isPickable()`.** `EntitySelector.CAN_BE_PICKED` is `Entity::isPickable`,
   which is overridden thirteen times with genuinely different bodies. The
   default is **false**, so an item entity, an experience orb and a display are
   never picked; `EnderDragon` overrides it back to **false** while its
   (unregistered) `EnderDragonPart`s return true. Neither is guessable from the
   registry name.

Extraction, all from the decompile (REWO_PLAN §11), never a wiki:

* `EntityTypes.java` — one `register(EntityTypeIds.NAME, EntityType.Builder…)`
  block per type; `.sized(w, h)` inside it, or the builder default when absent.
* `EntityTypeIds.java` — `NAME = create("cow")` gives the registry string.
* `EntityType.java` — the builder's own default dimensions, read rather than
  assumed.
* Every `net/minecraft/**/*.java` `class X extends Y`, walked to find the
  nearest ancestor that declares `isPickable()`.

Re-run after a version bump:

    python tools/gen_entity_pick.py

Fails loud rather than defaulting: an unparsed registration, an unresolved id
constant, a broken `extends` chain, a `.sized(` the block regex did not
attribute to a registration, an `isPickable()` declarer that is not in
[`PICK_BODIES`], or an override body that no longer matches the text this table
was derived from. In particular, if 26.3 changes `LivingEntity.isPickable()`
from `!isRemoved()` to something else, the generator stops here instead of
silently shipping the old rule.
"""
import os
import re
import sys

VERSION = "26.2"
DECOMP = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", VERSION, "decompiled")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "entity_pick_table.rs")

ENTITY_TYPES = os.path.join(DECOMP, "net/minecraft/world/entity/EntityTypes.java")
ENTITY_TYPE_IDS = os.path.join(DECOMP, "net/minecraft/world/entity/EntityTypeIds.java")
ENTITY_TYPE = os.path.join(DECOMP, "net/minecraft/world/entity/EntityType.java")

REGISTRATION = re.compile(
    r'public static final EntityType<([A-Za-z0-9_.]+)>\s+([A-Z0-9_]+)\s*=\s*register\(\s*'
    r'EntityTypeIds\.([A-Z0-9_]+)\s*,')
ID_CONST = re.compile(
    r'ResourceKey<EntityType<\?>>\s+([A-Z0-9_]+)\s*=\s*create\(\s*"([a-z0-9_/]+)"\s*\)')
CLASS_DECL = re.compile(
    r'^[ \t]*(?:public\s+|private\s+|protected\s+|static\s+|final\s+|abstract\s+'
    r'|sealed\s+|non-sealed\s+)*class\s+'
    r'([A-Za-z0-9_]+)(?:<[^{]*?>)?\s+extends\s+([A-Za-z0-9_.]+)',
    re.M)
SIZED = re.compile(r'\.sized\(\s*([0-9.]+)F\s*,\s*([0-9.]+)F\s*\)')
# `EntityDimensions dimensions = EntityDimensions.scalable(0.6F, 1.8F);` —
# the builder's field initialiser, i.e. what a registration without `.sized()`
# gets. Read rather than assumed.
BUILDER_DEFAULT = re.compile(
    r'EntityDimensions\s+dimensions\s*=\s*EntityDimensions\.scalable\(\s*'
    r'([0-9.]+)F\s*,\s*([0-9.]+)F\s*\)')
# `public boolean isPickable() {  return <body>;  }` — one statement in every
# 26.2 override, which is what makes an exact-text assertion possible.
IS_PICKABLE = re.compile(
    r'public boolean isPickable\(\)\s*\{\s*return\s+([^;]+);\s*\}', re.S)

ROOT = "Entity"

# The thirteen `isPickable()` bodies in 26.2, each mapped to the Rust rule that
# evaluates it. The body text is asserted verbatim: a rule silently changing
# meaning under the same class name is exactly the failure this catches.
#
# `!isRemoved()` is `Alive` rather than a constant because it is genuinely a
# per-entity question in vanilla; every entity in Rewo's table is un-removed by
# construction (removal deletes the row), which is recorded on the Rust side.
PICK_BODIES = {
    "Entity":              ("false", "Never"),
    "LivingEntity":        ("!this.isRemoved()", "Alive"),
    "Player":              ("!this.isSpectator() && super.isPickable()", "AliveUnlessSpectator"),
    "ArmorStand":          ("super.isPickable() && !this.isMarker()", "AliveUnlessMarker"),
    "EndCrystal":          ("true", "Always"),
    "EnderDragon":         ("false", "Never"),
    "EnderDragonPart":     ("true", "Always"),
    "BlockAttachedEntity": ("true", "Always"),
    "Interaction":         ("true", "Always"),
    "FallingBlockEntity":  ("!this.isRemoved()", "Alive"),
    "PrimedTnt":           ("!this.isRemoved()", "Alive"),
    "AbstractBoat":        ("!this.isRemoved()", "Alive"),
    "AbstractMinecart":    ("!this.isRemoved()", "Alive"),
    "Projectile":          ("this.is(EntityTypeTags.REDIRECTABLE_PROJECTILE)", "RedirectableProjectile"),
    "AbstractArrow":       ("super.isPickable() && !this.isInGround()", "RedirectableProjectileNotInGround"),
    "ShulkerBullet":       ("true", "Always"),
}

# `Entity.getPickRadius()` is 0.0F; only `Projectile` overrides it, to
# `isPickable() ? 1.0F : 0.0F`. Asserted below so a new override is caught.
EXPECTED_PICK_RADIUS_OVERRIDES = {
    "net/minecraft/world/entity/Entity.java",
    "net/minecraft/world/entity/projectile/Projectile.java",
}
# `Entity.canBePickedFromInside()` is true; only `SulfurCube` overrides it.
EXPECTED_INSIDE_OVERRIDES = {
    "net/minecraft/world/entity/Entity.java",
    "net/minecraft/world/entity/monster/cubemob/SulfurCube.java",
}

RULE_DOC = {
    "Never": [
        "`Entity.isPickable()` — a flat `false`, which is the **default** and",
        "therefore the answer for an item entity, an experience orb, a display,",
        "a lightning bolt and everything else that never overrode it. Also",
        "`EnderDragon`'s own override: the dragon's body is not pickable, only",
        "its (unregistered) `EnderDragonPart` hitboxes are.",
    ],
    "Always": [
        "A flat `true` — `EndCrystal`, `Interaction`, `ShulkerBullet` and",
        "`BlockAttachedEntity` (item frames, paintings, leash knots).",
    ],
    "Alive": [
        "`!this.isRemoved()` — `LivingEntity`, `PrimedTnt`,",
        "`FallingBlockEntity`, `AbstractBoat` and `AbstractMinecart`.",
    ],
    "AliveUnlessSpectator": [
        "`Player`: `!this.isSpectator() && super.isPickable()`. A spectating",
        "player is never under anyone's crosshair.",
    ],
    "AliveUnlessMarker": [
        "`ArmorStand`: `super.isPickable() && !this.isMarker()`.",
    ],
    "RedirectableProjectile": [
        "`Projectile`: `this.is(EntityTypeTags.REDIRECTABLE_PROJECTILE)` — a",
        "**tag**, resolved from the client jar's data pack, not from this table.",
    ],
    "RedirectableProjectileNotInGround": [
        "`AbstractArrow`: `super.isPickable() && !this.isInGround()`, so an",
        "arrow must be in the tag *and* still in flight.",
    ],
}


def die(msg):
    print(f"gen_entity_pick: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        return f.read()


def scan_class_graph():
    """-> ({class: superclass}, {class: {supers}}) — same two tiers as
    `gen_entity_classes.py`, because Vineflower writes nested entity
    implementations inline."""
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
                die(f"nested class {cur} has ambiguous supers {sorted(cands)}")
            cur = next(iter(cands))
    return None


def scan_overrides():
    """-> ({class: body-text}, {relpath: …}) for every `isPickable()` in the tree."""
    bodies, files = {}, set()
    radius, inside = set(), set()
    for root, _dirs, fnames in os.walk(os.path.join(DECOMP, "net")):
        for fname in fnames:
            if not fname.endswith(".java"):
                continue
            path = os.path.join(root, fname)
            rel = os.path.relpath(path, DECOMP).replace("\\", "/")
            text = read(path)
            if "getPickRadius()" in text and "public float getPickRadius()" in text:
                radius.add(rel)
            if "public boolean canBePickedFromInside()" in text:
                inside.add(rel)
            found = IS_PICKABLE.findall(text)
            if not found:
                continue
            cls = fname[: -len(".java")]
            if len(found) > 1:
                die(f"{rel}: {len(found)} isPickable() bodies in one file")
            bodies[cls] = " ".join(found[0].split())
            files.add(rel)
    return bodies, radius, inside


def main():
    for p in (ENTITY_TYPES, ENTITY_TYPE_IDS, ENTITY_TYPE):
        if not os.path.isfile(p):
            die(f"missing {p}")

    # 1. The builder's default dimensions, read from the source.
    m = BUILDER_DEFAULT.search(read(ENTITY_TYPE))
    if not m:
        die("EntityType.Builder's default `EntityDimensions dimensions = "
            "EntityDimensions.scalable(w, h)` initialiser no longer parses")
    default_w, default_h = float(m.group(1)), float(m.group(2))

    # 2. registry name -> (implementation class, width, height).
    ids = dict(ID_CONST.findall(read(ENTITY_TYPE_IDS)))
    src = read(ENTITY_TYPES)
    matches = list(REGISTRATION.finditer(src))
    total_register = len(re.findall(r'=\s*register\(', src))
    if len(matches) != total_register:
        die(f"parsed {len(matches)} of {total_register} `register(` calls in "
            "EntityTypes.java — a registration shape changed")
    by_name, unsized = {}, []
    sized_seen = 0
    for i, mm in enumerate(matches):
        cls, field, id_const = mm.group(1), mm.group(2), mm.group(3)
        if field != id_const:
            die(f"registration {field} names EntityTypeIds.{id_const}")
        name = ids.get(id_const)
        if name is None:
            die(f'EntityTypeIds.{id_const} has no create("…") literal')
        end = matches[i + 1].start() if i + 1 < len(matches) else len(src)
        block = src[mm.end():end]
        sizes = SIZED.findall(block)
        if len(sizes) > 1:
            die(f"{name}: {len(sizes)} `.sized(` calls in one registration block")
        if sizes:
            sized_seen += 1
            w, h = float(sizes[0][0]), float(sizes[0][1])
        else:
            unsized.append(name)
            w, h = default_w, default_h
        by_name["minecraft:" + name] = (cls.split(".")[-1], w, h)
    # Every `.sized(` in the file must belong to a registration block we parsed,
    # or a type is silently taking the builder default.
    total_sized = len(SIZED.findall(src))
    if sized_seen != total_sized:
        die(f"attributed {sized_seen} of {total_sized} `.sized(` calls to "
            "registrations — the block split is wrong")

    # 3. isPickable() overrides + the two constants that ride with them.
    bodies, radius_files, inside_files = scan_overrides()
    if radius_files != EXPECTED_PICK_RADIUS_OVERRIDES:
        die("the set of getPickRadius() overrides changed:\n"
            f"  expected {sorted(EXPECTED_PICK_RADIUS_OVERRIDES)}\n"
            f"  found    {sorted(radius_files)}")
    if inside_files != EXPECTED_INSIDE_OVERRIDES:
        die("the set of canBePickedFromInside() overrides changed:\n"
            f"  expected {sorted(EXPECTED_INSIDE_OVERRIDES)}\n"
            f"  found    {sorted(inside_files)}")
    for cls, body in bodies.items():
        entry = PICK_BODIES.get(cls)
        if entry is None:
            die(f"class {cls} declares isPickable() and has no rule — its body "
                f"is `{body}`. Add it to PICK_BODIES deliberately.")
        expect, _rule = entry
        if body != expect:
            die(f"{cls}.isPickable() body changed:\n  expected `{expect}`\n"
                f"  found    `{body}`")
    for cls in PICK_BODIES:
        if cls not in bodies:
            die(f"PICK_BODIES names {cls}, which no longer declares isPickable()")

    # 4. Walk each implementation class to its nearest isPickable() declarer.
    top, nested = scan_class_graph()
    rows, rule_counts = [], {}
    for name, (cls, w, h) in sorted(by_name.items()):
        path = chain(cls, top, nested)
        if path is None:
            die(f"{name}: class {cls} has no `extends` chain reaching {ROOT}")
        declarer = next((c for c in path if c in bodies), None)
        if declarer is None:
            die(f"{name}: no class in {path} declares isPickable() — not even "
                f"{ROOT}, which means the chain is broken")
        rule = PICK_BODIES[declarer][1]
        rows.append((name, w, h, rule, declarer))
        rule_counts[rule] = rule_counts.get(rule, 0) + 1

    order = sorted(RULE_DOC)
    with open(OUT, "w", encoding="utf-8", newline="\n") as out:
        out.write("//! GENERATED by `tools/gen_entity_pick.py` — do not edit.\n//!\n")
        out.write(f"//! Source: the decompiled {VERSION} client — `EntityTypes.java`\n")
        out.write("//! (`.sized(w, h)` per registration), `EntityType.java` (the builder's\n")
        out.write("//! own default dimensions), `EntityTypeIds.java`, and every\n")
        out.write("//! `isPickable()` declaration under `net/` walked against the\n")
        out.write("//! `class X extends Y` graph. Re-run after a version bump; the script\n")
        out.write("//! fails loud on an unparsed registration, an unattributed `.sized(`, an\n")
        out.write("//! `isPickable()` declarer with no rule, or an override body whose text\n")
        out.write("//! changed.\n//!\n")
        out.write(f"//! {len(rows)} registered types. Rule census: ")
        out.write(", ".join(f"{k} {rule_counts.get(k, 0)}" for k in order))
        out.write(".\n\n")
        out.write("/// Registered entity types in this version — a runtime registry of a\n")
        out.write("/// different size means the pin has drifted.\n")
        out.write(f"pub const REGISTERED_TYPES: usize = {len(rows)};\n\n")
        out.write("/// `EntityType.Builder`'s own default, used by the ")
        out.write(f"{len(unsized)} registration(s)\n")
        out.write("/// that call no `.sized(…)`")
        if unsized:
            out.write(" — " + ", ".join(unsized))
        out.write(".\n")
        out.write(f"pub const BUILDER_DEFAULT_SIZE: (f32, f32) = ({default_w}, {default_h});\n\n")
        out.write("/// Which `isPickable()` body a type inherits.\n")
        out.write("///\n")
        out.write("/// `EntitySelector.CAN_BE_PICKED` is `Entity::isPickable`, and the\n")
        out.write("/// default is **false** — so the interesting cases are the ones that are\n")
        out.write("/// *not* pickable, not the ones that are.\n")
        out.write("#[derive(Clone, Copy, Debug, PartialEq, Eq)]\n")
        out.write("pub enum PickRule {\n")
        for i, rule in enumerate(order):
            if i:
                out.write("\n")
            for line in RULE_DOC[rule]:
                out.write(f"    /// {line}\n")
            out.write(f"    {rule},\n")
        out.write("}\n\n")
        out.write("/// `(registry name, width, height, pick rule)` for every registered\n")
        out.write("/// type. Width and height are `EntityType.Builder.sized(w, h)`, the\n")
        out.write("/// arguments to `EntityDimensions.scalable`; the bounding box they make\n")
        out.write("/// is `AABB(x - w/2, y, z - w/2, x + w/2, y + h, z + w/2)`.\n")
        out.write("pub const ENTITY_PICK: &[(&str, f32, f32, PickRule)] = &[\n")
        for name, w, h, rule, declarer in rows:
            out.write(f'    ("{name}", {w}, {h}, PickRule::{rule}), // {declarer}\n')
        out.write("];\n")
    print(f"gen_entity_pick: {len(rows)} types "
          f"({len(unsized)} unsized), rules " +
          ", ".join(f"{k}={rule_counts.get(k, 0)}" for k in order) +
          f" -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
