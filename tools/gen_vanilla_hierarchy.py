"""Machine-extract vanilla's entity-model part hierarchies from the decompiled
Minecraft jar into a Rust table (`crates/rewo-gpu/src/vanilla_hier.rs`).

Why this exists
---------------
OptiFine CEM `.jem` top-level parts correspond to *vanilla* model parts, and
they inherit **vanilla's** parent hierarchy — not just the `.jem` nesting. A
pack states an animated part's translation relative to that vanilla parent, so
a part that vanilla nests under another (the vex's `right_arm` under `body`)
must be parented the same way here, or it flies off. Flat models (every part a
child of the root) need nothing — which is why most mobs already looked right.

Ground truth is the decompile, never community docs (REWO_PLAN §11). Re-run
after a version bump:

    python tools/gen_vanilla_hierarchy.py

Parsing: vanilla builds meshes with `PartDefinition x = y.addOrReplaceChild(
"name", ...)`. Track variable -> part-name bindings, then each call's receiver
names the parent. Both the single-line and wrapped forms appear, so the source
is whitespace-normalized first.
"""
import os
import re
import sys

DECOMP = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", "26.2",
                      "decompiled", "net", "minecraft", "client", "model")
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "..", "crates", "rewo-gpu", "src", "vanilla_hier.rs")

# `PartDefinition v =`? `recv` .addOrReplaceChild( "name"
CALL = re.compile(
    r'(?:PartDefinition\s+(\w+)\s*=\s*)?(\w+)\s*\.\s*addOrReplaceChild\(\s*"([A-Za-z_0-9]+)"'
)
ROOT_BIND = re.compile(r'PartDefinition\s+(\w+)\s*=\s*\w+\s*\.\s*getRoot\(\)')


def hierarchy_of(path):
    """-> {child_name: parent_name}; parent_name None means model root."""
    src = open(path, encoding="utf8", errors="replace").read()
    src = re.sub(r"\s+", " ", src)
    var2part = {}
    for m in ROOT_BIND.finditer(src):
        var2part[m.group(1)] = None  # the mesh root
    out = {}
    for m in CALL.finditer(src):
        assigned, recv, name = m.group(1), m.group(2), m.group(3)
        if recv not in var2part:
            # receiver is not a tracked PartDefinition (e.g. a builder) — skip
            continue
        out[name] = var2part[recv]
        if assigned:
            var2part[assigned] = name
    return out


def main():
    if not os.path.isdir(DECOMP):
        print("decompile not found:", DECOMP, file=sys.stderr)
        return 1
    models = {}
    for root, _dirs, files in os.walk(DECOMP):
        for f in files:
            if not f.endswith("Model.java"):
                continue
            h = hierarchy_of(os.path.join(root, f))
            # keep only models with real nesting (a child whose parent is a part)
            nested = {c: p for c, p in h.items() if p is not None}
            if nested:
                models[f[:-len("Model.java")]] = nested

    rows = []
    for cls in sorted(models):
        pairs = ", ".join('("%s", "%s")' % (c, p)
                          for c, p in sorted(models[cls].items()))
        rows.append('    ("%s", &[%s]),' % (snake(cls), pairs))

    with open(os.path.normpath(OUT), "w", encoding="utf8") as fh:
        fh.write(HEADER)
        fh.write("pub const VANILLA_HIERARCHY: &[(&str, &[(&str, &str)])] = &[\n")
        fh.write("\n".join(rows))
        fh.write("\n];\n")
    print("wrote %s — %d models with nesting" % (os.path.normpath(OUT), len(models)))
    return 0


def snake(s):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", s).lower()


HEADER = '''//! Vanilla entity-model part hierarchies — GENERATED, do not edit.
//!
//! Regenerate with `python tools/gen_vanilla_hierarchy.py` after a version
//! bump. Source of truth is the decompiled client jar (REWO_PLAN §11).
//!
//! OptiFine CEM `.jem` top-level parts map onto *vanilla* model parts and
//! inherit vanilla's parent hierarchy, not just the `.jem` nesting — a pack
//! states an animated part's translation relative to that vanilla parent. The
//! vex's `right_arm` is a child of `body` in `VexModel`, so FA writes
//! `right_arm.ty = 0.6` against the body; treating it as absolute put the arms
//! 18 px above the mob. Only models with real nesting are listed; flat models
//! (every part under the root) need no entry.

'''

if __name__ == "__main__":
    sys.exit(main())
