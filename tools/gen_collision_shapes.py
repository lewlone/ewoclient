#!/usr/bin/env python3
"""Extract block collision shapes from the decompiled block classes.

Vanilla stores collision in Java, not in any datagen report. The rule is
`BlockBehaviour.getCollisionShape`: `hasCollision ? getShape(..) : empty`,
overridable per class. This script resolves, per block, the first class in its
`extends` chain that overrides `getCollisionShape` (else `getShape`) and — when
that override's body is a single `return <constant expression>;` — evaluates
the expression (`Block.box/column/cube/boxZ`, `Shapes.or/block/empty`, and
static `VoxelShape` constants of the same class).

Emitted into `crates/rewo-data/src/collision_table.rs`:

* `NO_COLLISION` — registrations calling `noCollision()`.
* `COLLISION` — state-independent shapes in block-local 0..1 boxes: every
  block whose chain overrides neither method (the full cube) and every block
  whose override resolves to a constant.
* `STATE_DEPENDENT` — blocks whose override exists but is not a constant
  (stairs, panes, doors, chests, …): the bake falls back to model geometry.

Run after a version bump:
    python tools/gen_collision_shapes.py
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen_block_light as gbl  # noqa: E402
import gen_block_props as gbp  # noqa: E402

BLOCK_DIR = gbp.BLOCK_DIR
STOP = {"Block", "BlockBehaviour"}


def class_sources():
    out = {}
    for dirpath, _dirs, files in os.walk(BLOCK_DIR):
        for fn in files:
            if fn.endswith(".java"):
                out[fn[:-5]] = open(os.path.join(dirpath, fn), encoding="utf8", errors="replace").read()
    return out


def method_body(text, name):
    m = re.search(r"protected VoxelShape " + name + r"\s*\(", text) or re.search(
        r"public VoxelShape " + name + r"\s*\(", text)
    if not m:
        return None
    brace = text.find("{", m.end())
    depth, j = 0, brace
    while j < len(text):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                break
        j += 1
    return text[brace + 1 : j].strip()


class Unresolved(Exception):
    pass


def split_args(s):
    args, depth, cur = [], 0, ""
    for ch in s:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == "," and depth == 0:
            args.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        args.append(cur.strip())
    return args


def num(s):
    s = s.strip().rstrip("FfDd")
    try:
        return float(s)
    except ValueError:
        raise Unresolved(s)


def box16(x0, y0, z0, x1, y1, z1):
    return [(x0 / 16, y0 / 16, z0 / 16, x1 / 16, y1 / 16, z1 / 16)]


def evaluate(expr, consts, sources, cls, depth=0):
    expr = expr.strip()
    if depth > 20:
        raise Unresolved(expr)
    m = re.fullmatch(r"(?:Block\.)?(box|column|cube|boxZ)\((.*)\)", expr, re.S)
    if m:
        a = [num(x) for x in split_args(m.group(2))]
        f = m.group(1)
        if f == "box" and len(a) == 6:
            return box16(*a)
        if f == "column":
            sx, sz, y0, y1 = (a[0], a[0], a[1], a[2]) if len(a) == 3 else a
            return box16(8 - sx / 2, y0, 8 - sz / 2, 8 + sx / 2, y1, 8 + sz / 2)
        if f == "cube":
            sx, sy, sz = (a[0], a[0], a[0]) if len(a) == 1 else a
            return box16(8 - sx / 2, 8 - sy / 2, 8 - sz / 2, 8 + sx / 2, 8 + sy / 2, 8 + sz / 2)
        if f == "boxZ":
            if len(a) == 3:
                sx, sy, z0, z1 = a[0], a[0], a[1], a[2]
                return box16(8 - sx / 2, 8 - sy / 2, z0, 8 + sx / 2, 8 + sy / 2, z1)
            if len(a) == 4:
                sx, sy, z0, z1 = a
                return box16(8 - sx / 2, 8 - sy / 2, z0, 8 + sx / 2, 8 + sy / 2, z1)
            if len(a) == 5:
                sx, y0, y1, z0, z1 = a
                return box16(8 - sx / 2, y0, z0, 8 + sx / 2, y1, z1)
        raise Unresolved(expr)
    if re.fullmatch(r"Shapes\.block\(\)", expr):
        return [(0.0, 0.0, 0.0, 1.0, 1.0, 1.0)]
    if re.fullmatch(r"Shapes\.empty\(\)", expr):
        return []
    m = re.fullmatch(r"Shapes\.box\((.*)\)", expr, re.S)
    if m:
        a = [num(x) for x in split_args(m.group(1))]
        return [tuple(a)]
    m = re.fullmatch(r"Shapes\.or\((.*)\)", expr, re.S)
    if m:
        out = []
        for x in split_args(m.group(1)):
            out += evaluate(x, consts, sources, cls, depth + 1)
        return out
    m = re.fullmatch(r"(?:(\w+)\.)?([A-Z][A-Z0-9_]*)", expr)
    if m:
        owner = m.group(1) or cls
        table = consts.get(owner) or {}
        if m.group(2) in table:
            return evaluate(table[m.group(2)], consts, sources, owner, depth + 1)
    raise Unresolved(expr)


def scan_constants(sources):
    out = {}
    for cls, text in sources.items():
        table = {}
        for m in re.finditer(r"static final VoxelShape ([A-Z][A-Z0-9_]*) = ", text):
            end = text.find(";", m.end())
            table[m.group(1)] = text[m.end() : end]
        out[cls] = table
    return out


def resolve_class(cls, sups, sources, consts):
    """('shape', boxes) | ('state', None) — for a class chain."""
    for method in ("getCollisionShape", "getShape"):
        for c in gbp.chain(sups, cls):
            if c in STOP:
                break
            body = method_body(sources.get(c, ""), method)
            if body is None:
                continue
            m = re.fullmatch(r"return (.*);", " ".join(body.split()), re.S)
            if not m:
                return ("state", None)
            try:
                return ("shape", evaluate(m.group(1), consts, sources, c))
            except Unresolved:
                return ("state", None)
        # `getCollisionShape` not overridden: fall through to `getShape`.
    return ("shape", [(0.0, 0.0, 0.0, 1.0, 1.0, 1.0)])


def main():
    src = open(gbl.BLOCKS, encoding="utf8", errors="replace").read()
    sups = gbp.superclasses()
    sources = class_sources()
    consts = scan_constants(sources)
    id_tables = gbl.scan_id_tables()
    single_ids = gbl.scan_single_ids()
    known = set(json.load(open(gbl.REGISTRY, encoding="utf8")).keys())
    helpers = {}
    for m in re.finditer(r"(?:private|public)\s+static\s+BlockBehaviour\.Properties\s+(\w+)\s*\(", src):
        brace = src.find("{", m.end())
        depth, j = 0, brace
        while j < len(src):
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        helpers[m.group(1)] = src[brace : j + 1]

    # `private static Block registerStair(...) { return register(.., p -> new
    # StairBlock(..), ..); }` — registration helpers that name the class.
    block_helpers = {}
    for m in re.finditer(r"private\s+static\s+Block\s+(\w+)\s*\(", src):
        brace = src.find("{", m.end())
        depth, j = 0, brace
        while j < len(src):
            if src[j] == "{":
                depth += 1
            elif src[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        block_helpers[m.group(1)] = src[brace : j + 1]

    no_collision, shapes, state_dep = [], {}, []

    def plain_register(body):
        return re.match(r"\(\s*Block(?:Item)?Ids\.\w+\s*,\s*(?:BlockBehaviour\.)?Properties", body) is not None

    def record(name, body, cls, func=None):
        if f"minecraft:{name}" not in known:
            return
        expanded = body
        for hname, hbody in helpers.items():
            if hname + "(" in expanded:
                expanded += "\n" + hbody
        if cls is None:
            if func in block_helpers:
                hbody = block_helpers[func]
                expanded += "\n" + hbody
                cls = gbl.impl_class(hbody)
        if cls is None and not (func == "register" and plain_register(body)):
            # An unrecognised construction: never guess a full cube.
            state_dep.append(name)
            return
        overrides = any(
            method_body(sources.get(c, ""), "getCollisionShape") is not None
            for c in gbp.chain(sups, cls or "Block")
            if c not in STOP
        )
        # `noCollision()` only clears `hasCollision`, which the default
        # `getCollisionShape` reads; an override (scaffolding) ignores it.
        if "noCollision()" in expanded and not overrides:
            no_collision.append(name)
            return
        kind, boxes = resolve_class(cls or "Block", sups, sources, consts)
        if kind == "shape":
            shapes[name] = boxes
        else:
            state_dep.append(name)

    for m in re.finditer(r"public\s+static\s+final\s+Block\s+([A-Z0-9_]+)\s*=\s*(\w+)\s*\(", src, re.M):
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"Block(?:Item)?Ids\.(\w+)", body)
        name = single_ids.get(ref.group(1)) if ref else None
        record(name or m.group(1).lower(), body, gbl.impl_class(body), m.group(2))
    for m in re.finditer(
        r"public\s+static\s+final\s+(?:Color|WeatheringCopper)Collection<Block>\s+"
        r"[A-Z0-9_]+\s*=\s*\w+\s*\.\s*registerBlocks\s*\(", src, re.M
    ):
        body = gbl.balanced(src, m.end() - 1)
        ref = re.search(r"BlockItemIds\.(\w+)", body)
        names = id_tables.get(ref.group(1)) if ref else None
        if not names:
            continue
        cls = gbl.impl_class(body)
        for name, _ in names:
            record(name, body, cls)

    dest = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "crates", "rewo-data", "src", "collision_table.rs",
    )
    out = open(dest, "w", encoding="utf8", newline="\n")
    w = out.write
    w("//! GENERATED by `tools/gen_collision_shapes.py` — do not edit.\n//!\n")
    w(f"//! Source: decompiled {gbl.VERSION} block classes. See the script header.\n\n")
    w("/// Registrations calling `noCollision()`.\npub const NO_COLLISION: &[&str] = &[\n")
    for k in sorted(no_collision):
        w(f'    "minecraft:{k}",\n')
    w("];\n\n")
    w("/// State-independent collision shapes, block-local `[x0,y0,z0,x1,y1,z1]`.\n")
    w("pub const COLLISION: &[(&str, &[[f32; 6]])] = &[\n")
    for k in sorted(shapes):
        boxes = ", ".join("[" + ", ".join(repr(float(v)) for v in b) + "]" for b in shapes[k])
        w(f'    ("minecraft:{k}", &[{boxes}]),\n')
    w("];\n\n")
    w("/// Blocks whose collision depends on state or context (model fallback).\n")
    w("pub const STATE_DEPENDENT: &[&str] = &[\n")
    for k in sorted(state_dep):
        w(f'    "minecraft:{k}",\n')
    w("];\n")
    out.close()
    print(f"[gen_collision_shapes] {len(no_collision)} no-collision, {len(shapes)} constant, "
          f"{len(state_dep)} state-dependent", file=sys.stderr)


if __name__ == "__main__":
    main()
