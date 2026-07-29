"""Machine-extract every registered entity type's **attachment points** from
the decompiled 26.2 jar into `crates/rewo-data/src/entity_attachments_table.rs`.

Why this exists
---------------
M72 places a passenger on its vehicle. 26.x does not hold the seat position in
code the way older versions held a `getPassengersRidingOffset()` constant — it
holds it as **per-entity-type data**, declared on the `EntityType.Builder` and
baked into `EntityDimensions.attachments()`:

    EntityType.Builder.of(Pig::new, …).sized(0.9F, 0.9F).passengerAttachments(0.86875F)

`Entity.getPassengerRidingPosition` is then

    this.position().add(attachments.getClamped(PASSENGER, indexOf(passenger), this.yRot))

and `positionRider` subtracts the *passenger's own* VEHICLE point. So there are
two tables, one keyed by the vehicle's type and one by the rider's, and both
are data. Transcribing them by hand across 158 types would rot silently.

Extraction, all from the decompile (REWO_PLAN §11), never a wiki:

* `EntityTypes.java` — one `register(EntityTypeIds.X, EntityType.Builder…)`
  chain per type. Read `sized(w, h)`, `passengerAttachments(…)` in both its
  `float...` and `Vec3...` forms, `vehicleAttachment(Vec3)` and
  `ridingOffset(float)`.
* `EntityTypeIds.java` — `X = create("pig")` gives the registry string.
* Any `Cls.CONST` appearing inside those calls is resolved by reading
  `Cls.java`'s `static final Vec3 CONST = new Vec3(…)`.

Two conventions in `EntityType.Builder` that invert if you assume them:

* `passengerAttachments(float... offsetYs)` attaches `(0, offsetY, 0)` — the
  bare floats are **Y** offsets, and a type may declare several (the happy
  ghast declares four full `Vec3` seats).
* `ridingOffset(float r)` attaches VEHICLE at `(0, -r, 0)` — **negated**. A
  zombie's `ridingOffset(-0.7F)` is a VEHICLE point of `(0, +0.7, 0)`.

And two fallbacks, from `EntityAttachment`'s enum:

* PASSENGER falls back to `AT_HEIGHT` = `(0, height, 0)` — the top of the
  bounding box, which is why `sized` must be captured too.
* VEHICLE falls back to `AT_FEET` = `Vec3.ZERO`.

Re-run after a version bump:

    python tools/gen_entity_attachments.py

Fails loud rather than defaulting: a registration whose builder chain has no
`sized(...)`, an unresolved `EntityTypeIds` constant, an unresolved `Cls.CONST`
symbol, an unparseable numeric literal, a `Vec3` of other than three
components, or an attachment builder call this script does not know about are
all hard errors. In particular, if 26.3 adds a third attachment-declaring
builder method the generator stops here instead of shipping a table that
silently drops it.
"""
import os
import re
import sys

VERSION = "26.2"
DECOMP = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", VERSION, "decompiled")
NET = os.path.join(DECOMP, "net")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-data", "src", "entity_attachments_table.rs")

ENTITY_TYPES = os.path.join(DECOMP, "net/minecraft/world/entity/EntityTypes.java")
ENTITY_TYPE_IDS = os.path.join(DECOMP, "net/minecraft/world/entity/EntityTypeIds.java")

ID_CONST = re.compile(
    r'ResourceKey<EntityType<\?>>\s+([A-Z0-9_]+)\s*=\s*create\(\s*"([a-z0-9_/]+)"\s*\)')
# The head of one registration: `... = register(\n EntityTypeIds.PIG,`
REG_HEAD = re.compile(r'=\s*register\(\s*EntityTypeIds\.([A-Z0-9_]+)\s*,', re.S)
SIZED = re.compile(r'\.sized\(\s*([^,)]+?)\s*,\s*([^,)]+?)\s*\)')
NUM = re.compile(r'^-?\d+(?:\.\d+)?(?:[fFdD])?$')
VEC3 = re.compile(r'new\s+Vec3\(')
QUALIFIED = re.compile(r'^([A-Z][A-Za-z0-9_]*)\.([A-Z][A-Z0-9_]*)$')
# `public static final Vec3 DEFAULT_VEHICLE_ATTACHMENT = new Vec3(0.0, 0.6, 0.0);`
VEC3_CONST = re.compile(
    r'static\s+final\s+Vec3\s+{name}\s*=\s*new\s+Vec3\(([^;]*?)\)\s*;')

# Every builder method that writes an attachment point. `attach(...)` is the
# raw form; nothing in `EntityTypes.java` uses it today, but it is listed so
# an unknown call is distinguishable from an unused one.
ATTACH_CALLS = ("passengerAttachments", "vehicleAttachment", "ridingOffset",
                "nameTagOffset", "attach")
# The ones M72 consumes. `nameTagOffset` and `attach` are parsed only far
# enough to prove they are not silently carrying a PASSENGER/VEHICLE point.
CONSUMED = ("passengerAttachments", "vehicleAttachment", "ridingOffset")


def die(msg):
    print(f"gen_entity_attachments: {msg}", file=sys.stderr)
    sys.exit(1)


def read(path):
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        return f.read()


def find_class_file(cls):
    """Locate `<cls>.java` anywhere under `net/`. Ambiguity is a hard error."""
    hits = []
    for root, _dirs, files in os.walk(NET):
        if f"{cls}.java" in files:
            hits.append(os.path.join(root, f"{cls}.java"))
    if not hits:
        die(f"cannot resolve symbol {cls}.* — no {cls}.java under net/")
    if len(hits) > 1:
        die(f"ambiguous class {cls}: {hits}")
    return hits[0]


def num(tok, what):
    tok = tok.strip()
    if not NUM.match(tok):
        die(f"{what}: {tok!r} is not a numeric literal — the builder call takes "
            "a constant here and anything else would need evaluating")
    return float(tok.rstrip("fFdD"))


def args_of(text, open_idx):
    """Return the argument text of a call whose '(' is at `open_idx`."""
    depth = 0
    for i in range(open_idx, len(text)):
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                return text[open_idx + 1:i], i
    die("unbalanced parentheses in EntityTypes.java")


def split_top(text):
    """Split a call's arguments on top-level commas.

    Angle brackets are deliberately **not** tracked: `EntityTypes.java` is full
    of `() -> Items.OAK_BOAT` lambdas, whose `>` would close a depth that never
    opened. No registration carries a top-level generic argument list, so
    parenthesis depth alone is exact here.
    """
    out, depth, cur = [], 0, ""
    for ch in text:
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur)
    return [a.strip() for a in out if a.strip()]


def vec3(expr, what):
    """Evaluate a `new Vec3(a, b, c)` or a `Cls.CONST` naming one."""
    expr = expr.strip()
    m = VEC3.match(expr)
    if m:
        inner, end = args_of(expr, m.end() - 1)
        if expr[end + 1:].strip():
            die(f"{what}: trailing text after new Vec3(...): {expr!r}")
        parts = split_top(inner)
        if len(parts) != 3:
            die(f"{what}: new Vec3 with {len(parts)} components: {expr!r}")
        return tuple(num(p, what) for p in parts)
    q = QUALIFIED.match(expr)
    if q:
        cls, const = q.group(1), q.group(2)
        src = read(find_class_file(cls))
        cm = re.search(VEC3_CONST.pattern.format(name=re.escape(const)), src)
        if not cm:
            die(f"{what}: {expr} is not a `static final Vec3 = new Vec3(...)` "
                f"in {cls}.java")
        parts = split_top(cm.group(1))
        if len(parts) != 3:
            die(f"{what}: {expr} resolves to a Vec3 with {len(parts)} components")
        return tuple(num(p, what) for p in parts)
    die(f"{what}: cannot evaluate {expr!r} as a Vec3")


def blocks(src):
    """Yield (id-constant, builder-chain text) for every `register(...)`."""
    for m in REG_HEAD.finditer(src):
        # The '(' of `register(` is the last one before the match end.
        open_idx = src.rindex("(", m.start(), m.end())
        body, _end = args_of(src, open_idx)
        parts = split_top(body)
        if len(parts) != 2:
            die(f"{m.group(1)}: register() with {len(parts)} arguments")
        yield m.group(1), parts[1]


def parse(chain, const):
    """Extract (width, height, passenger points, vehicle point) from one chain."""
    sized = SIZED.search(chain)
    if not sized:
        die(f"{const}: builder chain has no .sized(w, h) — the PASSENGER "
            "fallback is AT_HEIGHT and cannot be resolved without it")
    width = num(sized.group(1), f"{const}.sized width")
    height = num(sized.group(2), f"{const}.sized height")

    passenger, vehicle = [], None
    for call in re.finditer(r'\.([A-Za-z0-9_]+)\(', chain):
        name = call.group(1)
        if name not in ATTACH_CALLS:
            continue
        inner, _end = args_of(chain, call.end() - 1)
        args = split_top(inner)
        if name == "passengerAttachments":
            for a in args:
                if VEC3.match(a) or QUALIFIED.match(a):
                    passenger.append(vec3(a, f"{const}.passengerAttachments"))
                else:
                    # `passengerAttachments(float... offsetYs)` — a Y offset.
                    passenger.append((0.0, num(a, f"{const}.passengerAttachments"), 0.0))
        elif name == "vehicleAttachment":
            if len(args) != 1:
                die(f"{const}: vehicleAttachment with {len(args)} arguments")
            if vehicle is not None:
                die(f"{const}: two VEHICLE attachment points — `get(VEHICLE, 0, …)` "
                    "only ever reads index 0, so a second one would be silently lost")
            vehicle = vec3(args[0], f"{const}.vehicleAttachment")
        elif name == "ridingOffset":
            if len(args) != 1:
                die(f"{const}: ridingOffset with {len(args)} arguments")
            if vehicle is not None:
                die(f"{const}: two VEHICLE attachment points")
            # `attach(VEHICLE, 0.0F, -ridingOffset, 0.0F)` — NEGATED.
            vehicle = (0.0, -num(args[0], f"{const}.ridingOffset"), 0.0)
        elif name == "nameTagOffset":
            num(args[0], f"{const}.nameTagOffset")  # parsed, not consumed
        elif name == "attach":
            # The raw form. 26.2 uses it exactly once, for the warden's
            # WARDEN_CHEST (the sonic-boom origin), which M72 does not read.
            # A PASSENGER or VEHICLE point arriving this way would bypass the
            # named helpers above and be silently dropped, so it is fatal.
            which = args[0].strip()
            if not which.startswith("EntityAttachment."):
                die(f"{const}: .attach() with a non-literal attachment: {which!r}")
            kind = which.split(".", 1)[1]
            if kind not in ("NAME_TAG", "WARDEN_CHEST"):
                die(f"{const}: raw .attach(EntityAttachment.{kind}, …) — M72 reads "
                    "PASSENGER and VEHICLE through the named builder helpers, so "
                    "this point would be dropped")
    return width, height, passenger, vehicle


def main():
    ids = dict(ID_CONST.findall(read(ENTITY_TYPE_IDS)))
    if not ids:
        die("parsed no id constants from EntityTypeIds.java")
    src = read(ENTITY_TYPES)

    rows, n_pass, n_veh = [], 0, 0
    for const, chain in blocks(src):
        if const not in ids:
            die(f"EntityTypeIds.{const} has no create(\"…\") — unresolved id")
        width, height, passenger, vehicle = parse(chain, const)
        if passenger:
            n_pass += 1
        if vehicle is not None:
            n_veh += 1
        rows.append((f"minecraft:{ids[const]}", width, height, passenger, vehicle))
    if not rows:
        die("parsed no registrations from EntityTypes.java")
    dupes = {n for n, *_ in rows if [r[0] for r in rows].count(n) > 1}
    if dupes:
        die(f"duplicate registry names: {sorted(dupes)}")
    rows.sort(key=lambda r: r[0])

    def f(v):
        s = repr(float(v))
        return s

    with open(OUT, "w", encoding="utf-8", newline="\n") as out:
        out.write("//! GENERATED by `tools/gen_entity_attachments.py` — do not edit.\n//!\n")
        out.write(f"//! Source: the decompiled {VERSION} client\n")
        out.write("//! (`EntityTypes.java` builder chains + `EntityTypeIds.java`). Re-run\n")
        out.write("//! after a version bump; the script fails loud on a chain with no\n")
        out.write("//! `sized(...)`, an unresolved id or `Cls.CONST` symbol, a non-literal\n")
        out.write("//! argument, or an attachment builder method it does not know.\n//!\n")
        out.write(f"//! {len(rows)} registered types, {n_pass} declaring PASSENGER points, "
                  f"{n_veh} declaring a VEHICLE point.\n\n")
        out.write("/// Registered entity types in this version — a runtime registry of a\n")
        out.write("/// different size means the pin has drifted.\n")
        out.write(f"pub const SCANNED_TYPES: usize = {len(rows)};\n\n")
        out.write("/// One entity type's attachment declaration.\n")
        out.write("///\n")
        out.write("/// `passenger` empty means the type declares none, and\n")
        out.write("/// `EntityAttachment.PASSENGER`'s `AT_HEIGHT` fallback applies —\n")
        out.write("/// `(0, height, 0)`, which is why `height` is carried. `vehicle` `None`\n")
        out.write("/// likewise means the `AT_FEET` fallback, `Vec3.ZERO`.\n")
        out.write("pub struct TypeAttachments {\n")
        out.write("    pub name: &'static str,\n")
        out.write("    /// `sized(width, …)` — the bounding-box width.\n")
        out.write("    pub width: f32,\n")
        out.write("    /// `sized(…, height)` — also the PASSENGER fallback point's Y.\n")
        out.write("    pub height: f32,\n")
        out.write("    /// Declared PASSENGER points, in declaration order. The Nth\n")
        out.write("    /// passenger takes the Nth, **clamped** to the last.\n")
        out.write("    pub passenger: &'static [[f64; 3]],\n")
        out.write("    /// The declared VEHICLE point — this type's own offset when it is\n")
        out.write("    /// the *rider*. `ridingOffset(r)` lands here as `(0, -r, 0)`.\n")
        out.write("    pub vehicle: Option<[f64; 3]>,\n")
        out.write("}\n\n")
        out.write("/// Every registered type, sorted by registry name.\n")
        out.write("pub const TYPES: &[TypeAttachments] = &[\n")
        for name, width, height, passenger, vehicle in rows:
            pts = ", ".join(f"[{f(x)}, {f(y)}, {f(z)}]" for x, y, z in passenger)
            veh = ("None" if vehicle is None
                   else f"Some([{f(vehicle[0])}, {f(vehicle[1])}, {f(vehicle[2])}])")
            out.write(f'    TypeAttachments {{ name: "{name}", width: {f(width)}, '
                      f"height: {f(height)}, passenger: &[{pts}], vehicle: {veh} }},\n")
        out.write("];\n")
    print(f"gen_entity_attachments: {len(rows)} types, {n_pass} with passenger points, "
          f"{n_veh} with a vehicle point -> {os.path.normpath(OUT)}")


if __name__ == "__main__":
    main()
