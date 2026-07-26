"""Extract `CopperGolemModel`'s four statue pose layers into Rust.

`CopperGolemStatueBlockRenderer` bakes FOUR separate layers — STANDING,
RUNNING, SITTING and STAR — and they are not one model posed: each is its own
`LayerDefinition` with its own nested `PartDefinition` tree, where a child's
offset rides through its parent's rotation. Roughly thirty-five boxes across
the four, most carrying a rotation to four decimal places.

Hand-transcribing that is exactly the kind of work whose errors are silent: a
statue with one arm a few degrees off still looks like a statue. So it is
machine-extracted, the same way `gen_anim_defs`, `gen_block_light` and
`gen_vanilla_hierarchy` are, and re-run after a version bump.

Usage (from the repo root):

    python tools/gen_copper_golem_poses.py > crates/rewo-data/src/copper_golem_poses.rs

Emits deterministic LF so a re-run reproduces the file byte-for-byte and
`git diff --check` stays clean.
"""

import io
import os
import re
import sys

SRC = (
    r"%APPDATA%/EwoClient/rewo/26.2/decompiled/net/minecraft/client/model/"
    r"animal/golem/CopperGolemModel.java"
)

LAYERS = [
    ("STANDING", "createBodyLayer"),
    ("RUNNING", "createRunningPoseBodyLayer"),
    ("SITTING", "createSittingPoseBodyLayer"),
    ("STAR", "createStarPoseBodyLayer"),
]

NUM = r"(-?[\d.]+)F?"


def f(x):
    return f"{float(x):.6}"


def layer_body(src, method):
    """The text of one `createXLayer` method."""
    i = src.index(f"public static LayerDefinition {method}()")
    depth, j = 0, src.index("{", i)
    k = j
    while True:
        if src[k] == "{":
            depth += 1
        elif src[k] == "}":
            depth -= 1
            if depth == 0:
                return src[j : k + 1]
        k += 1


def parse_pose(text):
    """`PartPose.offset(...)` / `.offsetAndRotation(...)` / `.ZERO`."""
    m = re.search(
        r"PartPose\.offsetAndRotation\(\s*%s,\s*%s,\s*%s,\s*%s,\s*%s,\s*%s\s*\)"
        % ((NUM,) * 6),
        text,
    )
    if m:
        g = m.groups()
        return (g[0], g[1], g[2]), (g[3], g[4], g[5])
    m = re.search(r"PartPose\.offset\(\s*%s,\s*%s,\s*%s\s*\)" % ((NUM,) * 3), text)
    if m:
        return m.groups(), ("0", "0", "0")
    return ("0", "0", "0"), ("0", "0", "0")


def parse_boxes(text):
    """Every `texOffs(u,v)` + following `addBox(...)` pair, with deformation.

    A `CubeListBuilder` chains `texOffs` before each `addBox`, so the offset in
    force for a box is the nearest preceding one — which is why this walks the
    two token streams together rather than zipping two independent findall
    results.
    """
    out = []
    tex = ("0", "0")
    pattern = re.compile(
        r"texOffs\(\s*%s,\s*%s\s*\)|addBox\(\s*%s,\s*%s,\s*%s,\s*%s,\s*%s,\s*%s"
        r"(?:\s*,\s*(?:new CubeDeformation\(\s*%s\s*\)|CubeDeformation\.NONE))?\s*\)"
        % ((NUM,) * 9),
        re.S,
    )
    for m in pattern.finditer(text):
        g = m.groups()
        if g[0] is not None:
            tex = (g[0], g[1])
        else:
            grow = g[8] if g[8] is not None else "0"
            out.append((tex, g[2:5], g[5:8], grow))
    return out


def parse_layer(body):
    """Flatten one layer into `(chain, own_pose, boxes)` rows.

    Each `addOrReplaceChild` call is matched to its parent by the receiver of
    the call (`root.` / `body.` / `right_arm.`), and the local variable a
    `PartDefinition` is assigned to names that part. That is enough structure
    for these four layers, which never reuse a name across depths.
    """
    # The mesh root may carry its own translation, which the STANDING layer
    # uses and the other three do not.
    m = re.search(r"transformed\(p -> p\.translated\(\s*%s,\s*%s,\s*%s\s*\)\)" % ((NUM,) * 3), body)
    root_pose = (m.groups(), ("0", "0", "0")) if m else (("0", "0", "0"), ("0", "0", "0"))

    parent_of = {}
    pose_of = {"root": root_pose}
    rows = []

    call = re.compile(
        r"(?:PartDefinition\s+(\w+)\s*=\s*)?(\w+)\.addOrReplaceChild\(\s*\"(\w+)\",(.*?)\n      \);",
        re.S,
    )
    for m in call.finditer(body):
        var, parent, name, args = m.group(1), m.group(2), m.group(3), m.group(4)
        key = var or name
        parent_of[key] = parent
        pose_of[key] = parse_pose(args)
        boxes = parse_boxes(args)
        if boxes:
            rows.append((key, boxes))

    def chain(key):
        """Ancestor poses, outermost first, excluding the part's own."""
        out, cur = [], parent_of.get(key, "root")
        while True:
            out.append(pose_of.get(cur, (("0",) * 3, ("0",) * 3)))
            if cur == "root":
                break
            cur = parent_of.get(cur, "root")
        return list(reversed(out))

    return [(chain(k), pose_of[k], boxes) for k, boxes in rows]


def main():
    path = os.path.expandvars(SRC)
    if not os.path.exists(path):
        sys.exit(f"decompiled source not found: {path}")
    src = io.open(path, encoding="utf-8").read()

    o = []
    w = o.append
    w("//! GENERATED by `tools/gen_copper_golem_poses.py` — DO NOT EDIT.")
    w("//!")
    w("//! `CopperGolemModel`'s four statue pose layers, flattened from their")
    w("//! nested `PartDefinition` trees. Re-run the generator after a version")
    w("//! bump; hand-editing this file will be silently undone.")
    w("//!")
    w("//! Each row is `(ancestor chain outermost-first, own pose, boxes)`. A")
    w("//! child's offset rides through its parent's ROTATION, not just its")
    w("//! translation, which is why the chain is kept rather than pre-summed.")
    w("")
    w("use super::block_entity_models::StatueBox;")
    w("")
    w("/// One `PartPose`: an offset and a rest rotation in radians.")
    w("pub type Pose = ([f32; 3], [f32; 3]);")
    w("")

    names = []
    for const, method in LAYERS:
        rows = parse_layer(layer_body(src, method))
        total = sum(len(b) for _, _, b in rows)
        names.append(const)
        w(f"/// `{method}` — {len(rows)} parts, {total} boxes.")
        w(f"pub const {const}: &[StatueBox] = &[")
        for ch, own, boxes in rows:
            chain_lit = ", ".join(
                "([%s, %s, %s], [%s, %s, %s])"
                % (f(p[0][0]), f(p[0][1]), f(p[0][2]), f(p[1][0]), f(p[1][1]), f(p[1][2]))
                for p in ch
            )
            own_lit = "([%s, %s, %s], [%s, %s, %s])" % (
                f(own[0][0]), f(own[0][1]), f(own[0][2]),
                f(own[1][0]), f(own[1][1]), f(own[1][2]),
            )
            for tex, mn, dm, grow in boxes:
                w("    StatueBox {")
                w(f"        tex: ({f(tex[0])}, {f(tex[1])}),")
                w(f"        min: [{f(mn[0])}, {f(mn[1])}, {f(mn[2])}],")
                w(f"        dims: [{f(dm[0])}, {f(dm[1])}, {f(dm[2])}],")
                w(f"        grow: {f(grow)},")
                w(f"        own: {own_lit},")
                w(f"        chain: &[{chain_lit}],")
                w("    },")
        w("];")
        w("")

    w("/// The four poses, in `CopperGolemStatueBlock.Pose` declaration order.")
    w("pub const POSES: &[(&str, &[StatueBox])] = &[")
    for n in names:
        w(f'    ("{n.lower()}", {n}),')
    w("];")
    w("")

    sys.stdout.buffer.write(("\n".join(o)).encode("utf-8"))


if __name__ == "__main__":
    main()
