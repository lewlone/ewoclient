"""Machine-extract, from the decompiled 26.2 jar, the two attribute tables an
`update_attributes` packet is resolved against — into
`crates/rewo-data/src/entity_attributes.rs`.

Why this exists
---------------
M52 decodes `ClientboundUpdateAttributesPacket`, which carries a *base value*
and a modifier list but nothing else. Turning that into a number needs two
facts the wire never sends and no datagen report carries:

1. **The per-attribute clamp.** `AttributeInstance.calculateValue` ends in
   `this.attribute.value().sanitizeValue(result)`, and every one of the 40
   registered attributes is a `RangedAttribute`, whose `sanitizeValue` is
   `NaN ? min : Mth.clamp(value, min, max)`. `max_health` is
   `RangedAttribute("attribute.name.max_health", 20.0, 1.0, 1024.0)` — so a
   server that sends base 0 resolves to **1.0**, not 0.

2. **The per-entity-type default set.** `ClientPacketListener
   .handleUpdateAttributes` looks the attribute up with
   `AttributeMap.getInstance`, which returns null — "warn and skip" — when the
   entity's `AttributeSupplier` does not declare it. So the supplier is not
   only where an *unmentioned* attribute's base comes from, it is also the
   filter deciding which attributes an entity can hold at all. A zombie has
   `spawn_reinforcements_chance`; a pig does not.

`DefaultAttributes.SUPPLIERS` is an `ImmutableMap` of ~90 entries, each
`EntityTypes.X -> SomeClass.createAttributes().build()`, and those builders
chain through the class hierarchy (`Zombie` -> `Monster` -> `Mob` ->
`LivingEntity`), so the table is spread over ~85 files. Hand-copying it would
fail silently, which is the same reason `gen_copper_golem_poses.py` exists.

Two details that a careless transcription gets wrong
----------------------------------------------------
* **`add()` twice keeps the LAST.** `AttributeSupplier.Builder.build()` calls
  `ImmutableMap.Builder.buildKeepingLast()`, so `Zombie`'s
  `.add(Attributes.MOVEMENT_SPEED, 0.23F)` overrides the `LivingEntity` entry
  it inherited rather than throwing. Order of application is load-bearing.

* **A float literal is widened, not rounded.** Vanilla writes
  `.add(Attributes.MOVEMENT_SPEED, 0.23F)` — a `float` promoted to `double`,
  which is `0.23000000298023224`, not `0.23`. Parsing `0.23F` as a Python
  float loses that. Every `F`/`f`-suffixed literal is round-tripped through
  IEEE-754 binary32 here, the same class of detail as M37's `+ 0.1F`.

Re-run after a version bump. The script fails loud on an unparsed
registration, an unknown attribute constant, an unresolvable class, a builder
whose chain it cannot follow, or a numeric literal it does not recognise.
"""

import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DECOMPILED = os.environ.get(
    "REWO_DECOMPILED",
    os.path.join(os.environ.get("APPDATA", ""), "EwoClient", "rewo", "26.2",
                 "decompiled"))
OUT = os.path.join(HERE, "..", "crates", "rewo-data", "src",
                   "entity_attributes.rs")

ENTITY_ROOT = os.path.join("net", "minecraft", "world", "entity")
ATTRS_JAVA = os.path.join(ENTITY_ROOT, "ai", "attributes", "Attributes.java")
DEFAULTS_JAVA = os.path.join(ENTITY_ROOT, "ai", "attributes",
                             "DefaultAttributes.java")
IDS_JAVA = os.path.join(ENTITY_ROOT, "EntityTypeIds.java")


def die(msg):
    print(f"gen_entity_attributes: FATAL: {msg}", file=sys.stderr)
    sys.exit(1)


def read(rel):
    path = os.path.join(DECOMPILED, rel)
    if not os.path.isfile(path):
        die(f"missing decompiled source {path}")
    with open(path, "r", encoding="utf-8") as handle:
        return handle.read()


def flatten(text):
    """Collapse the decompiler's line wrapping so one statement is one line."""
    return re.sub(r"\s+", " ", text)


# --------------------------------------------------------------------------
# 1. The attribute registry: name -> (default, min, max, syncable).
# --------------------------------------------------------------------------

def parse_attributes():
    src = read(ATTRS_JAVA)
    flat = flatten(src)
    # register("max_health", new RangedAttribute("attribute.name.max_health",
    #          20.0, 1.0, 1024.0).setSyncable(true))
    # The constant name is NOT the registry name: `SPAWN_REINFORCEMENTS_CHANCE`
    # registers as `"spawn_reinforcements"`. Both are captured so a builder's
    # `Attributes.CONST` resolves through the real mapping.
    pattern = re.compile(
        r'Holder<Attribute> ([A-Z_0-9]+) = register\(\s*"([a-z0-9_]+)"\s*,\s*'
        r'new RangedAttribute\(\s*'
        r'"([^"]+)"\s*,\s*([^,]+?)\s*,\s*([^,]+?)\s*,\s*([^,)]+?)\s*\)'
        r'((?:\s*\.set\w+\([^)]*\))*)')
    out = {}
    const_to_name = {}
    for m in pattern.finditer(flat):
        const, name, desc, default, lo, hi, tail = m.groups()
        const_to_name[const] = name
        out[name] = {
            "description": desc,
            "default": java_double(default, f"{name} default"),
            "min": java_double(lo, f"{name} min"),
            "max": java_double(hi, f"{name} max"),
            "syncable": ".setSyncable(true)" in tail.replace(" ", ""),
        }
    # Every registration in the file must have parsed. `register(` also names
    # the private helper's own declaration, so count the assignments instead.
    declared = len(re.findall(r"Holder<Attribute> [A-Z_0-9]+ = register\(", flat))
    if declared != len(out):
        die(f"{ATTRS_JAVA}: parsed {len(out)} of {declared} registrations")
    if not out:
        die(f"{ATTRS_JAVA}: no attributes parsed")
    # Only RangedAttribute is handled; a plain Attribute would have no clamp
    # and `sanitizeValue` would become polymorphic.
    others = set(re.findall(r"new (\w*Attribute)\(", flat)) - {"RangedAttribute"}
    if others:
        die(f"{ATTRS_JAVA}: unhandled attribute class(es) {sorted(others)}")
    return out, const_to_name


def java_double(literal, what):
    """Java numeric literal -> the exact double the JVM would store."""
    text = literal.strip()
    if not re.fullmatch(r"[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?[FfDdLl]?", text):
        die(f"unrecognised numeric literal {literal!r} ({what})")
    suffix = text[-1]
    if suffix in "FfDdLl":
        text = text[:-1]
    value = float(text)
    if suffix in "Ff":
        # A float literal widened to double: round through binary32 first.
        value = struct.unpack("<f", struct.pack("<f", value))[0]
    return value


# --------------------------------------------------------------------------
# 2. EntityTypes constant -> registry name.
# --------------------------------------------------------------------------

def parse_entity_ids():
    src = read(IDS_JAVA)
    out = dict(re.findall(
        r"ResourceKey<EntityType<\?>> ([A-Z_0-9]+) = create\(\"([a-z0-9_]+)\"\)",
        src))
    if not out:
        die(f"{IDS_JAVA}: no id constants parsed")
    declared = len(re.findall(r"ResourceKey<EntityType<\?>> [A-Z_0-9]+ =", src))
    if declared != len(out):
        die(f"{IDS_JAVA}: parsed {len(out)} of {declared} constants")
    return out


# --------------------------------------------------------------------------
# 3. Class resolution: a simple name -> its decompiled file, via imports.
# --------------------------------------------------------------------------

class Classes:
    def __init__(self):
        self._cache = {}

    def imports_of(self, rel):
        src = read(rel)
        table = {}
        for fq in re.findall(r"^import (?:static )?([\w.]+);", src, re.M):
            table[fq.rsplit(".", 1)[-1]] = fq
        return table

    def superclass_of(self, rel):
        """The `extends` target of the public class declared in `rel`, if any."""
        simple = os.path.basename(rel)[:-5]
        m = re.search(
            r"\b(?:public\s+)?(?:abstract\s+)?class\s+" + re.escape(simple) +
            r"\b[^{]*?\bextends\s+(\w+)", read(rel))
        return m.group(1) if m else None

    def declares(self, rel, method):
        return re.search(
            r"public static AttributeSupplier\.Builder " + re.escape(method) +
            r"\(\s*\)", read(rel)) is not None

    def find_static(self, method, from_rel):
        """An unqualified static call resolves up the `extends` chain."""
        rel, hops = from_rel, 0
        while rel is not None and hops < 32:
            if self.declares(rel, method):
                return rel
            sup = self.superclass_of(rel)
            rel = self.resolve(sup, rel) if sup else None
            hops += 1
        die(f"cannot find static {method}() from {from_rel} or any superclass")

    def resolve(self, simple, from_rel):
        """Locate `simple` as referenced from the file `from_rel`."""
        key = (simple, from_rel)
        if key in self._cache:
            return self._cache[key]
        imports = self.imports_of(from_rel)
        fq = imports.get(simple)
        if fq:
            rel = os.path.join(*fq.split(".")) + ".java"
            if os.path.isfile(os.path.join(DECOMPILED, rel)):
                self._cache[key] = rel
                return rel
        # Not imported: same package.
        sibling = os.path.join(os.path.dirname(from_rel), simple + ".java")
        if os.path.isfile(os.path.join(DECOMPILED, sibling)):
            self._cache[key] = sibling
            return sibling
        die(f"cannot resolve class {simple} referenced from {from_rel}")


# --------------------------------------------------------------------------
# 4. Builder chains.
# --------------------------------------------------------------------------

ADD_RE = re.compile(
    r"\.add\(\s*Attributes\.([A-Z_0-9]+)\s*(?:,\s*([^,)]+?)\s*)?\)")
BUILDER_RE = re.compile(r"AttributeSupplier\s*\.\s*builder\(\s*\)")
BASE_RE = re.compile(r"return\s+(?:(\w+)\s*\.\s*)?(\w+)\(\s*\)")


def method_body(rel, method):
    """The `return ...;` expression of a static builder method."""
    src = read(rel)
    m = re.search(
        r"public static AttributeSupplier\.Builder " + re.escape(method) +
        r"\(\s*\)\s*\{(.*?)\n   \}", src, re.S)
    if not m:
        die(f"{rel}: no builder method {method}()")
    body = m.group(1)
    if body.count("return") != 1:
        die(f"{rel}:{method}(): expected exactly one return, "
            f"got {body.count('return')} — the chain is not a plain expression")
    return body


def chain(rel, method, classes, consts, seen):
    """Resolve a builder method to an ordered list of (attribute, value|None).

    `value` is None for the one-argument `.add(attr)` form, which takes the
    attribute's own default.
    """
    key = (rel, method)
    if key in seen:
        die(f"builder cycle at {rel}:{method}()")
    seen = seen | {key}

    body = method_body(rel, method)
    flat = flatten(body)

    adds = []
    for m in ADD_RE.finditer(flat):
        const, value = m.group(1), m.group(2)
        name = consts.get(const)
        if name is None:
            die(f"{rel}:{method}(): unknown attribute Attributes.{const}")
        adds.append((name, None if value is None
                     else java_double(value, f"{rel}:{method} {const}")))

    # Every `.add(` in the body must have been one of the two recognised forms.
    if flat.count(".add(") != len(adds):
        die(f"{rel}:{method}(): {flat.count('.add(')} .add( calls but "
            f"{len(adds)} parsed — an unrecognised argument form")

    if BUILDER_RE.search(flat):
        return adds
    base = BASE_RE.search(flat)
    if not base:
        die(f"{rel}:{method}(): neither AttributeSupplier.builder() nor a base "
            f"call — cannot follow the chain")
    base_class, base_method = base.groups()
    # A static may be *inherited*: `DefaultAttributes` calls
    # `Cow.createAttributes()` though `AbstractCow` declares it, and
    # `ArmorStand` calls `createLivingAttributes()` unqualified. Both resolve
    # by walking the `extends` chain from the named class.
    start = rel if base_class is None else classes.resolve(base_class, rel)
    base_rel = classes.find_static(base_method, start)
    return chain(base_rel, base_method, classes, consts, seen) + adds


def parse_defaults(classes, attrs, consts, ids):
    src = read(DEFAULTS_JAVA)
    flat = flatten(src)
    puts = re.findall(
        r"\.put\(\s*EntityTypes\.([A-Z_0-9]+)\s*,\s*(\w+)\s*\.\s*(\w+)\(\s*\)"
        r"\s*\.\s*build\(\s*\)\s*\)", flat)
    total = len(re.findall(r"\.put\(\s*EntityTypes\.", flat))
    if len(puts) != total:
        die(f"{DEFAULTS_JAVA}: parsed {len(puts)} of {total} SUPPLIERS entries")
    if not puts:
        die(f"{DEFAULTS_JAVA}: no SUPPLIERS entries parsed")

    out = []
    for const, cls, method in puts:
        name = ids.get(const)
        if not name:
            die(f"{DEFAULTS_JAVA}: EntityTypes.{const} has no EntityTypeIds entry")
        rel = classes.find_static(
            method, classes.resolve(cls, DEFAULTS_JAVA))
        adds = chain(rel, method, classes, consts, frozenset())
        # buildKeepingLast(): a repeated attribute keeps the LAST value.
        resolved = {}
        for attr, value in adds:
            resolved[attr] = attrs[attr]["default"] if value is None else value
        out.append((f"minecraft:{name}", sorted(resolved.items())))
    out.sort()
    return out


# --------------------------------------------------------------------------
# 5. Emit.
# --------------------------------------------------------------------------

def rust_f64(value):
    text = repr(float(value))
    return text if ("." in text or "e" in text or "inf" in text) else text + ".0"


def main():
    attrs, consts = parse_attributes()
    ids = parse_entity_ids()
    classes = Classes()
    defaults = parse_defaults(classes, attrs, consts, ids)

    if "max_health" not in attrs:
        die("max_health missing from the attribute registry")
    mh = attrs["max_health"]
    if (mh["default"], mh["min"], mh["max"]) != (20.0, 1.0, 1024.0):
        die(f"max_health is {mh} — the pin has drifted")

    lines = []
    add = lines.append
    add("//! GENERATED by `tools/gen_entity_attributes.py` — do not edit.")
    add("//!")
    add("//! Source: the decompiled 26.2 client (`Attributes.java`,")
    add("//! `DefaultAttributes.java` + every `createAttributes()` builder it")
    add("//! chains through, `EntityTypeIds.java`). Re-run after a version")
    add("//! bump; the script fails loud on an unparsed registration, an")
    add("//! unknown attribute constant, an unfollowable builder chain or an")
    add("//! unrecognised numeric literal.")
    add("//!")
    add(f"//! {len(attrs)} attributes, {len(defaults)} entity types with a")
    add("//! default attribute supplier.")
    add("")
    add("/// One registered `minecraft:attribute`.")
    add("///")
    add("/// Every attribute in 26.2 is a `RangedAttribute`, so the clamp is")
    add("/// not optional: `sanitizeValue` is `NaN ? min : clamp(v, min, max)`")
    add("/// and it runs on the *result* of every resolution.")
    add("#[derive(Clone, Copy, Debug, PartialEq)]")
    add("pub struct AttrDef {")
    add("    /// Registry name without the `minecraft:` prefix.")
    add("    pub name: &'static str,")
    add("    /// `Attribute.getDefaultValue()` — the base an")
    add("    /// `AttributeSupplier.Builder.add(attr)` with no explicit value")
    add("    /// installs.")
    add("    pub default: f64,")
    add("    /// `RangedAttribute.getMinValue()`.")
    add("    pub min: f64,")
    add("    /// `RangedAttribute.getMaxValue()`.")
    add("    pub max: f64,")
    add("    /// `Attribute.isClientSyncable()` — false means a vanilla server")
    add("    /// never puts it in `getAttributesToSync()`, so receiving one is")
    add("    /// not an error but is worth knowing.")
    add("    pub syncable: bool,")
    add("}")
    add("")
    add("/// Every registered attribute, sorted by name.")
    add("pub const ATTRIBUTES: &[AttrDef] = &[")
    for name in sorted(attrs):
        a = attrs[name]
        add(f'    AttrDef {{ name: "{name}", default: {rust_f64(a["default"])}, '
            f'min: {rust_f64(a["min"])}, max: {rust_f64(a["max"])}, '
            f'syncable: {"true" if a["syncable"] else "false"} }},')
    add("];")
    add("")
    add("/// Per-entity-type default attribute suppliers, sorted by registry")
    add("/// name; each entry's list is sorted by attribute name.")
    add("///")
    add("/// This is both the source of a base value the packet has not")
    add("/// covered **and** the filter on what an entity may hold at all:")
    add("/// `AttributeMap.getInstance` returns null for an attribute the")
    add("/// supplier does not declare, and `handleUpdateAttributes` logs")
    add("/// `\"Entity {} does not have attribute {}\"` and skips it.")
    add("pub const ENTITY_DEFAULTS: &[(&str, &[(&str, f64)])] = &[")
    for name, pairs in defaults:
        add(f'    ("{name}", &[')
        for attr, value in pairs:
            add(f'        ("{attr}", {rust_f64(value)}),')
        add("    ]),")
    add("];")
    add("")

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8", newline="\n") as handle:
        handle.write("\n".join(lines))

    print(f"gen_entity_attributes: {len(attrs)} attributes, "
          f"{len(defaults)} entity suppliers -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
