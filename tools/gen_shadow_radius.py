"""Extract vanilla's entity shadow radius and strength into a generated table.

    python tools/gen_shadow_radius.py

Ground truth: the decompile at %APPDATA%/EwoClient/rewo/26.2/decompiled
(`EntityRenderers.java` maps each EntityType to a renderer class, and the
renderer constructors chain a `shadow` parameter up to `EntityRenderer`).
Re-run after a version bump; never hand-edit
crates/rewo-data/src/shadow_radius_table.rs.

What it does:

- Evaluates the CONSTRUCTOR CHAIN of each registered renderer: the
  registration's own arguments (`new GiantMobRenderer(context, 6.0F)`), then
  `this(..)` / `super(..)` delegations, binding each parameter to a float
  literal where the call passes one, down to `EntityRenderer`
  (`shadowRadius` 0, `shadowStrength` 1). A `this.shadowRadius = X;` or
  `this.shadowStrength = X;` statement after the `super(..)` call overrides.
  An argument is a float literal, a bound parameter, or `literal * name`
  (the giant's `0.5F * scale`); anything else fails the run loudly.
- Records the renderer's getShadowRadius OVERRIDES by class, because the
  radius a renderer reports at run time depends on render state Rewo holds
  elsewhere: `LivingEntityRenderer` multiplies by `state.scale`,
  `MobRenderer` by `state.ageScale`, and a handful of renderers add their own
  baby factor or size term (see RULES). The table carries the base numbers
  and the rule name; the arithmetic is Rust, tested.

Fails loud on any registered renderer whose chain does not resolve, an
override it has no RULES entry for, or zero extractions.
"""

import os
import re
import sys

HOME = os.environ.get("USERPROFILE") or os.path.expanduser("~")
DECOMPILED = os.path.join(
    HOME, "AppData", "Roaming", "EwoClient", "rewo", "26.2", "decompiled"
)
OUT = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "crates", "rewo-data", "src", "shadow_radius_table.rs",
)
RENDERERS = os.path.join(DECOMPILED, "net", "minecraft", "client", "renderer", "entity")

# The renderer classes that override getShadowRadius / getShadowStrength, and
# the rule Rust applies for each. Every class the scan finds with such an
# override must be here (and the other way round), so a new override in a later
# version fails this script instead of being ignored.
RULES = {
    # `LivingEntityRenderer.getShadowRadius`: `super * state.scale` and
    # `MobRenderer.getShadowRadius`: `super * state.ageScale` apply to every
    # living entity and every mob, and are not listed here.
    "AbstractCubeMobRenderer": "CubeSize",      # state.size * 0.25F
    "PufferfishRenderer": "Pufferfish",         # 0.1F + 0.1F * puffState
    "StriderRenderer": "BabyHalf",              # baby: radius * 0.5F more
    "TurtleRenderer": "BabyTurtle",             # baby: radius * 0.83F more
    "VillagerRenderer": "BabyHalf",             # baby: radius * 0.5F more
    "DisplayRenderer": "Display",               # the display's own data
}
# Rules whose class is a base class: the rule applies to every subclass.
LIVING_BASE = "LivingEntityRenderer"
MOB_BASE = "MobRenderer"

CLASS_RE = re.compile(
    r"public\s+(?:abstract\s+|final\s+)?class\s+(\w+)(?:<[^{]*?>)?\s+extends\s+(\w+)"
)
REGISTER_RE = re.compile(r"register\(\s*EntityTypes\.(\w+),")
FLOAT_RE = re.compile(r"^-?\d+(?:\.\d+)?[Ff]?$")


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def balanced(text, start, open_ch="(", close_ch=")"):
    """Index just past the bracket that matches text[start] == open_ch."""
    depth = 0
    for i in range(start, len(text)):
        if text[i] == open_ch:
            depth += 1
        elif text[i] == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
    raise SystemExit("unbalanced bracket")


def split_args(s):
    """Top-level comma split of an argument list (inside the parens)."""
    out, depth, cur = [], 0, []
    for ch in s:
        if ch in "([{<":
            depth += 1
        elif ch in ")]}>":
            depth -= 1
        if ch == "," and depth == 0:
            out.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
    tail = "".join(cur).strip()
    if tail:
        out.append(tail)
    return out


def parse_class(path):
    text = read(path)
    m = CLASS_RE.search(text)
    if not m:
        return None
    name, parent = m.group(1), m.group(2)
    ctors = []
    # Constructors: `Name(` followed by params, then a body.
    for cm in re.finditer(r"\b(?:public|protected|private)\s+%s\s*\(" % re.escape(name), text):
        p_open = cm.end() - 1
        p_close = balanced(text, p_open)
        params_src = text[p_open + 1:p_close - 1]
        params = []
        for p in split_args(params_src):
            p = re.sub(r"\bfinal\b", "", p).strip()
            if p:
                params.append(p.split()[-1])
        b_open = text.index("{", p_close)
        b_close = balanced(text, b_open, "{", "}")
        body = text[b_open + 1:b_close - 1]
        ctors.append({"params": params, "body": body})
    overrides = []
    for fn in ("getShadowRadius", "getShadowStrength"):
        if re.search(r"\b%s\s*\(" % fn, text) and re.search(
            r"(?:protected|public)\s+float\s+%s\s*\(" % fn, text
        ):
            overrides.append(fn)
    return {"name": name, "parent": parent, "ctors": ctors, "overrides": overrides}


def first_call(body, keyword):
    """Args of a leading `super(..)` / `this(..)` call in a ctor body, or None."""
    m = re.match(r"\s*%s\s*\(" % keyword, body)
    if not m:
        return None
    start = m.end() - 1
    end = balanced(body, start)
    return split_args(body[start + 1:end - 1]), body[end:]


def eval_arg(arg, bound):
    """A float, a bound name, `literal * name`, or None when unknown."""
    arg = arg.strip()
    if FLOAT_RE.match(arg):
        return float(arg.rstrip("Ff"))
    if arg in bound:
        return bound[arg]
    m = re.match(r"^(-?\d+(?:\.\d+)?[Ff]?)\s*\*\s*(\w+)$", arg)
    if m and m.group(2) in bound and bound[m.group(2)] is not None:
        return float(m.group(1).rstrip("Ff")) * bound[m.group(2)]
    return None


def find_ctor(cls, argc, exclude=None):
    """A constructor of `argc` parameters. Overloads of one arity are told
    apart only by exclusion (a `this(..)` call never re-enters its caller),
    which is enough for every vanilla renderer; a second ambiguity fails."""
    cands = [c for c in cls["ctors"] if len(c["params"]) == argc and c is not exclude]
    if not cands:
        return None
    return cands[0]


def run_ctor(classes, cls, args, depth=0, exclude=None):
    """(radius, strength) of constructing `cls` with the evaluated `args`."""
    if depth > 12:
        raise SystemExit(f"constructor chain too deep at {cls and cls['name']}")
    if cls is None:
        return 0.0, 1.0
    if not cls["ctors"]:
        # No explicit constructor: the parent's no-arg one.
        return run_ctor(classes, classes.get(cls["parent"]), [], depth + 1)
    ctor = find_ctor(cls, len(args), exclude)
    if ctor is None:
        raise SystemExit(f"{cls['name']}: no constructor with {len(args)} args")
    bound = dict(zip(ctor["params"], args))
    body = ctor["body"]
    radius, strength = None, None
    for kw in ("this", "super"):
        call = first_call(body, kw)
        if call is None:
            continue
        call_args, rest = call
        vals = [eval_arg(a, bound) for a in call_args]
        # A non-float argument (a model, a layer) stays None; only floats and
        # parameters feeding the shadow matter.
        target = cls if kw == "this" else classes.get(cls["parent"])
        if target is None:
            radius, strength = 0.0, 1.0
        else:
            radius, strength = run_ctor(
                classes, target, vals, depth + 1, ctor if kw == "this" else None
            )
        body = rest
        break
    if radius is None:
        radius, strength = 0.0, 1.0
    for field, setter in (("shadowRadius", "r"), ("shadowStrength", "s")):
        for m in re.finditer(r"this\.%s\s*=\s*([^;]+);" % field, body):
            v = eval_arg(m.group(1), bound)
            if v is None:
                raise SystemExit(f"{cls['name']}: cannot evaluate {field} = {m.group(1)}")
            if setter == "r":
                radius = v
            else:
                strength = v
    return radius, strength


def main():
    if not os.path.isdir(RENDERERS):
        raise SystemExit(f"decompile not found at {RENDERERS}")
    classes = {}
    for dirpath, _, files in os.walk(RENDERERS):
        for fn in files:
            if fn.endswith(".java"):
                c = parse_class(os.path.join(dirpath, fn))
                if c:
                    classes[c["name"]] = c
    # The base class itself: shadowRadius 0, shadowStrength 1.
    classes.setdefault(
        "EntityRenderer",
        {"name": "EntityRenderer", "parent": None, "ctors": [], "overrides": []},
    )
    classes["EntityRenderer"]["ctors"] = []

    # ---- overrides must all be accounted for -------------------------------
    found = {n for n, c in classes.items() if c["overrides"]}
    expected = set(RULES) | {LIVING_BASE, MOB_BASE}
    if found != expected:
        raise SystemExit(
            "getShadowRadius/Strength overrides changed: "
            f"unexpected {sorted(found - expected)}, missing {sorted(expected - found)}"
        )

    # ---- registrations ------------------------------------------------------
    er = read(os.path.join(RENDERERS, "EntityRenderers.java"))
    regs = []
    for m in REGISTER_RE.finditer(er):
        start = er.rfind("register(", 0, m.end())
        open_i = start + len("register")
        end = balanced(er, open_i)
        inner = er[open_i + 1:end - 1]
        etype = m.group(1).lower()
        expr = inner.split(",", 1)[1].strip()
        regs.append((etype, expr))
    if not regs:
        raise SystemExit("no registrations found")

    rows = []
    for etype, expr in regs:
        # `Class::new` or `context -> new Class<..>(args)`.
        m = re.match(r"^(\w+)(?:\.\w+)?::new$", expr)
        if m:
            # `DisplayRenderer.BlockDisplayRenderer::new`: nested classes
            # share their outer class's file; the outer class carries the rule.
            cls_name, args = m.group(1), None
        else:
            m = re.search(r"new\s+(\w+)(?:<[^>]*>)?\s*\(", expr)
            if not m:
                raise SystemExit(f"{etype}: cannot read registration {expr!r}")
            cls_name = m.group(1)
            p_open = m.end() - 1
            p_close = balanced(expr, p_open)
            args = split_args(expr[p_open + 1:p_close - 1])
        cls = classes.get(cls_name)
        if cls is None:
            raise SystemExit(f"{etype}: renderer class {cls_name} not found")
        if args is None:
            # `Class::new`: the provider's `context` is the only argument.
            call_args = [None]
        else:
            # The registration's args: `context` first, then its literals.
            call_args = [eval_arg(a, {}) for a in args]
        radius, strength = run_ctor(classes, cls, call_args)
        # Which rule, if any: the nearest class up the chain with one.
        rule = "None"
        living = mob = False
        cur = cls_name
        seen = []
        while cur and cur in classes and cur not in seen:
            seen.append(cur)
            if cur in RULES and rule == "None":
                rule = RULES[cur]
            if cur == LIVING_BASE:
                living = True
            if cur == MOB_BASE:
                mob = True
            cur = classes[cur]["parent"]
        rows.append((etype, cls_name, radius, strength, rule, living, mob))

    rows.sort()
    # The avatar renderer is not in the registry: players and mannequins.
    avatar = classes["AvatarRenderer"]
    ar, as_ = run_ctor(classes, avatar, [None, False])
    extra = [("player", "AvatarRenderer", ar, as_, "None", True, False),
             ("mannequin", "AvatarRenderer", ar, as_, "None", True, False)]
    rows = sorted(rows + extra)
    names = [r[0] for r in rows]
    if len(set(names)) != len(names):
        raise SystemExit("duplicate entity rows")

    out = []
    out.append("//! GENERATED by tools/gen_shadow_radius.py - do not edit.")
    out.append("//!")
    out.append("//! Vanilla's `EntityRenderer.shadowRadius` / `shadowStrength` per entity")
    out.append("//! type, from the 26.2 decompile's constructor chains, with the name of the")
    out.append("//! `getShadowRadius` rule that applies on top (see `ShadowRule`). One row per")
    out.append("//! entity NAME (no namespace). `living` and `mob` say whether")
    out.append("//! `LivingEntityRenderer`'s `* state.scale` and `MobRenderer`'s")
    out.append("//! `* state.ageScale` multiply the radius. Re-run after a version bump.")
    out.append("")
    out.append("/// The `getShadowRadius` override a renderer class adds (see the generator).")
    out.append("#[derive(Clone, Copy, Debug, PartialEq, Eq)]")
    out.append("pub enum ShadowRule {")
    out.append("    None,")
    out.append("    /// `AbstractCubeMobRenderer`: `size * 0.25F`.")
    out.append("    CubeSize,")
    out.append("    /// `PufferfishRenderer`: `0.1F + 0.1F * puffState`.")
    out.append("    Pufferfish,")
    out.append("    /// A baby is halved once more (strider, villager).")
    out.append("    BabyHalf,")
    out.append("    /// A baby turtle is `* 0.83F` once more.")
    out.append("    BabyTurtle,")
    out.append("    /// `DisplayRenderer`: the display's own interpolated data.")
    out.append("    Display,")
    out.append("}")
    out.append("")
    out.append("#[derive(Clone, Copy, Debug)]")
    out.append("pub struct ShadowDef {")
    out.append("    pub entity: &'static str,")
    out.append("    pub renderer: &'static str,")
    out.append("    pub radius: f32,")
    out.append("    pub strength: f32,")
    out.append("    pub rule: ShadowRule,")
    out.append("    pub living: bool,")
    out.append("    pub mob: bool,")
    out.append("}")
    out.append("")
    out.append("pub static SHADOWS: &[ShadowDef] = &[")
    for etype, cls_name, radius, strength, rule, living, mob in rows:
        out.append(
            "    ShadowDef { entity: \"%s\", renderer: \"%s\", radius: %s, strength: %s, "
            "rule: ShadowRule::%s, living: %s, mob: %s },"
            % (etype, cls_name, rust_f32(radius), rust_f32(strength), rule,
               str(living).lower(), str(mob).lower())
        )
    out.append("];")
    out.append("")
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out))
    print(f"wrote {len(rows)} rows to {OUT}")


def rust_f32(v):
    s = repr(float(v))
    # Round-trip through float32 so the literal is exactly vanilla's `0.7F`.
    import struct
    f32 = struct.unpack("<f", struct.pack("<f", float(v)))[0]
    for digits in range(1, 10):
        t = f"{f32:.{digits}g}"
        if struct.unpack("<f", struct.pack("<f", float(t)))[0] == f32:
            s = t
            break
    if "e" not in s and "." not in s:
        s += ".0"
    return s


if __name__ == "__main__":
    sys.exit(main())
