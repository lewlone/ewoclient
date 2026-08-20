"""M164's mutation battery — waterlogged blocks render their water.

Run: python tools/m161_mutate.py [lo] [hi]

Discipline per AGENT_LOOP_BRIEF and REWO_PLAN §0.0:

  * a no-op control that must SURVIVE, or every KILLED below is vacuous;
  * exit codes and the gate's own summary line, never a substring of the body;
  * a per-mutation timeout, so a hang is a KILL rather than an outage whose
    `finally` never runs and leaves the mutant on disk;
  * a REBUILD after every restore — a gate-routed battery otherwise grades the
    previous mutant's BINARY against a clean tree;
  * the restore verified by BYTES, not by `git diff`, which cannot tell a
    leftover mutation from uncommitted work.

**Extended after review (M164b).** Three mutations were added because an
adversarial reviewer found them ALIVE against the shipped branch: forcing every
carrier onto `block/lava_still`, and swapping the pre-tinted and raw layers in
each of the two places they travel. `CarriedFluid::layer` and `raw_layer` had
no witness anywhere — `check_carried_fluid_table` read only `is_some`,
`falling`, `level` and `self_occludes`, `meshshot`'s oracle hand-builds
`layer: 1`, and `r48` counts cells. They are graded now by three
`blockentityshot` rows (the layer NAMES, the #3F76E4 tint arithmetic, and
agreement with `RenderKind::Fluid`) and one `rewo-mesh` test with distinct
sentinels.

**One mutation is routed at `live --render-check`, and its verdict is read per
WITNESS rather than from the exit code.** That is a deliberate exception with a
measurement behind it: `r46` (music) FAILS on `main` at `b88f18e` as well as on
this branch — measured, 46/47 there and 47/48 here — so the run's exit code is 1
either way and cannot distinguish anything. `r48` is the witness under test, so
its own PASS/FAIL line is what this reads. Every other mutation reads an exit
code.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MESH = os.path.join(ROOT, "crates/rewo-mesh/src/lib.rs")
DATA = os.path.join(ROOT, "crates/rewo-data/src/assets.rs")
LIVE = os.path.join(ROOT, "crates/rewo-app/src/live_cmd.rs")
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")

# (name, file, find, replace, checkers, what it would mean)
MUTATIONS = [
    (
        "control: no change",
        MESH,
        "const SELF_OCCLUDE_REMAP: [u32; 6] = [3, 2, 4, 5, 0, 1];",
        "const SELF_OCCLUDE_REMAP: [u32; 6] = [3, 2, 4, 5, 0, 1];",
        ["unit-mesh"],
        "MUST SURVIVE — otherwise every verdict below is vacuous",
    ),
    # ---- the rule: which states carry water -----------------------------
    (
        "carried_water: the unconditional five are keyed off `waterlogged` too",
        DATA,
        """    if UNCONDITIONAL_WATER.contains(&block_name) {""",
        """    if UNCONDITIONAL_WATER.contains(&block_name) && waterlogged {""",
        ["unit-data", "blockentityshot"],
        "kelp, kelp_plant, seagrass, tall_seagrass and every bubble column go "
        "dry — 32 states, and a conduit inside a kelp forest stops activating",
    ),
    (
        "carried_water: no waterlogged block carries falling water",
        DATA,
        "    waterlogged.then(|| FALLING_WATERLOGGED.contains(&block_name))",
        "    waterlogged.then_some(false)",
        ["unit-data", "blockentityshot"],
        "the eight copper grates lose FALLING — invisible until a flow "
        "milestone, which is exactly why it is recorded with a test",
    ),
    (
        "carried_water: a waterlogged state carries nothing",
        DATA,
        "    waterlogged.then(|| FALLING_WATERLOGGED.contains(&block_name))",
        "    let _ = waterlogged;\n    None",
        ["unit-data", "blockentityshot"],
        "11,728 of the 11,760 carriers vanish — the whole feature, minus the five",
    ),
    (
        "bake: a full occluding cube's carrier mask is face_occludes' 0",
        DATA,
        """                let self_occludes = if full_cube && !no_occlude.contains(block_name.as_str()) {
                    0b11_1111
                } else {
                    face_occludes[id as usize]
                };""",
        "                let self_occludes = face_occludes[id as usize];",
        ["blockentityshot"],
        "a waterlogged `type=double` slab (68 states) draws four side faces and "
        "a floor INSIDE solid stone",
    ),
    (
        "bake: isWaterAt forgets the carriers",
        DATA,
        """        *w = fluid[id].is_some()
            || matches!(render[id], RenderKind::Fluid { lava: false, .. });""",
        "        *w = matches!(render[id], RenderKind::Fluid { lava: false, .. });",
        ["blockentityshot"],
        "M30's conduit scan goes back to refusing a frame built out of "
        "waterlogged blocks, and kelp stops counting as water",
    ),
    (
        "bake: the carried level is not a source",
        DATA,
        """            fluid[id] = Some(CarriedFluid {
                layer,
                raw_layer,
                level: 0,""",
        """            fluid[id] = Some(CarriedFluid {
                layer,
                raw_layer,
                level: 3,""",
        ["blockentityshot"],
        "every carried surface sits at 5/9 instead of 8/9 — a flowing height for "
        "a fluid all 46 overrides create with `getSource`",
    ),
    # ---- the lookup ------------------------------------------------------
    (
        "fluid_at: the carried table is never consulted",
        MESH,
        "    let f = (*carried.get(state as usize)?)?;",
        "    if true { return None; }\n    let f = (*carried.get(state as usize)?)?;",
        ["unit-mesh", "meshshot"],
        "no waterlogged block renders water at all — the pre-M164 state",
    ),
    (
        "fluid_at: the face-order remap is the identity",
        MESH,
        "const SELF_OCCLUDE_REMAP: [u32; 6] = [3, 2, 4, 5, 0, 1];",
        "const SELF_OCCLUDE_REMAP: [u32; 6] = [0, 1, 2, 3, 4, 5];",
        ["unit-mesh", "meshshot"],
        "a bottom slab's DOWN occlusion is read as the mesher's face 2 (north), "
        "so the wrong side is suppressed and the floor is drawn inside the slab",
    ),
    (
        "fluid_level: a carried fluid is not the same fluid as a pool",
        MESH,
        """    let f = fluid_at(table, carried, world.block_state_at(x, y, z))?;
    (f.lava == want_lava).then_some(f.level)""",
        """    let f = fluid_at(table, carried, world.block_state_at(x, y, z))?;
    if f.carried {
        return None;
    }
    (f.lava == want_lava).then_some(f.level)""",
        ["unit-mesh", "meshshot"],
        "`isNeighborSameFluid` stops seeing waterlogged blocks: every pool draws "
        "a wall against one, and the waterlogged block draws one back",
    ),
    # ---- the faces -------------------------------------------------------
    (
        "emit_fluid: the self-occlusion test is dropped from the sides",
        MESH,
        """        if same(nx, y, nz)
            || f.self_occludes & (1 << face) != 0
            || is_full_cube(table, world.block_state_at(nx, y, nz))""",
        """        if same(nx, y, nz)
            || is_full_cube(table, world.block_state_at(nx, y, nz))""",
        ["unit-mesh", "meshshot"],
        "`shouldRenderFace`'s second half is gone; a waterlogged double slab "
        "draws four side faces inside itself",
    ),
    (
        "emit_fluid: the self-occlusion test is dropped from the bottom",
        MESH,
        """    if !same(wx, y - 1, wz)
        && f.self_occludes & (1 << 1) == 0
        && !is_full_cube(table, world.block_state_at(wx, y - 1, wz))""",
        """    if !same(wx, y - 1, wz)
        && !is_full_cube(table, world.block_state_at(wx, y - 1, wz))""",
        ["unit-mesh", "meshshot"],
        "a bottom slab's water gets a floor coplanar with the slab's own",
    ),
    (
        "emit_fluid: the self-occlusion test is applied to the TOP face too",
        MESH,
        """    // Top — unless the same fluid sits above.
    if !same(wx, y + 1, wz) {""",
        """    // Top — unless the same fluid sits above.
    if !same(wx, y + 1, wz) && f.self_occludes & 1 == 0 {""",
        ["unit-mesh", "meshshot"],
        "`renderUp` is the ONE face that skips `shouldRenderFace` "
        "(FluidRenderer:77); a top slab and every double slab lose their surface",
    ),
    # ---- the two draws ---------------------------------------------------
    (
        "mesh_column_reference: the fluid draw is dropped",
        MESH,
        """                    // `SectionCompiler.compile:89-97` — the fluid first, then
                    // the block, as two independent draws at one position.
                    if let Some(f) = fluid_at(table, carried, state) {""",
        """                    // `SectionCompiler.compile:89-97` — the fluid first, then
                    // the block, as two independent draws at one position.
                    if let Some(f) = fluid_at(table, carried, state).filter(|_| false) {""",
        ["unit-mesh", "meshshot"],
        "the frozen oracle stops seeing fluids, so the byte-identity controls "
        "grade the optimized mesher against a mesher that draws less",
    ),
    (
        "mesh_column: carried_fluid_cells never counts",
        MESH,
        """                        carried_fluid_cells +=
                            u32::from(f.carried && fv.len() > fluid_verts_before);
                        bump(y as f32);
                    }
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            for face in 0..6 {""",
        """                        bump(y as f32);
                    }
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            for face in 0..6 {""",
        ["unit-mesh", "meshshot"],
        "`r48` loses its only input and the windowed client stops being askable",
    ),
    (
        "mesh_column: carried_fluid_cells counts a cell that emitted nothing",
        MESH,
        """                        carried_fluid_cells +=
                            u32::from(f.carried && fv.len() > fluid_verts_before);
                        bump(y as f32);
                    }
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            for face in 0..6 {""",
        """                        let _ = fluid_verts_before;
                        carried_fluid_cells += u32::from(f.carried);
                        bump(y as f32);
                    }
                    match table.get(state as usize) {
                        Some(RenderKind::Cube {
                            faces,
                            raw_faces,
                            tint,
                        }) => {
                            for face in 0..6 {""",
        ["unit-mesh"],
        "the M164-as-shipped form: `r48`'s label says a block \"meshed its "
        "water\" while the counter fires for a cell whose six faces were all "
        "suppressed. Killed by "
        "`a_fully_occluded_submerged_carrier_is_not_counted_as_meshed`, which "
        "is the only fixture where the two forms can disagree — every other "
        "one emits at least the top face, which `renderUp` never suppresses",
    ),
    (
        "bake: every carrier samples `block/lava_still`",
        DATA,
        """            if !lava {
                water_layers = Some((layer, raw_layer));
            }""",
        """            if lava {
                water_layers = Some((layer, raw_layer));
            }""",
        ["unit-data", "blockentityshot", "meshshot"],
        "EVERY waterlogged block in the game renders orange lava. This is the "
        "review's measured hole: before `blockentityshot`'s three layer "
        "witnesses it left 185/185, rewo-data 231, meshshot and tintshot all "
        "green, i.e. the whole feature could have shipped pointing at the "
        "wrong sprite",
    ),
    (
        "bake: the pre-tinted and raw carried layers are swapped",
        DATA,
        """            fluid[id] = Some(CarriedFluid {
                layer,
                raw_layer,
                level: 0,""",
        """            fluid[id] = Some(CarriedFluid {
                layer: raw_layer,
                raw_layer: layer,
                level: 0,""",
        ["unit-data", "blockentityshot"],
        "a no-biome world double-tints its waterlogged water and a real world "
        "un-tints it — and the NAME witness cannot see it, because both layers "
        "are `block/water_still`; the #3F76E4 arithmetic is what catches it",
    ),
    (
        "fluid_at: the pre-tinted and raw layers are swapped on the way out",
        MESH,
        """    Some(FluidHere {
        layer: f.layer,
        raw_layer: f.raw_layer,""",
        """    Some(FluidHere {
        layer: f.raw_layer,
        raw_layer: f.layer,""",
        ["unit-mesh", "meshshot"],
        "the same swap one crate over, invisible to every pre-existing fixture "
        "because `oracle_waterlogged_carried` builds both layers as 1 — "
        "`fluid_at_keeps_the_pre_tinted_and_raw_layers_apart` is the witness",
    ),
    # ---- the wiring the windowed client alone can see --------------------
    (
        "live_cmd: MeshTables.fluid is left empty",
        LIVE,
        "        fluid: baked.fluid.clone(),",
        "        fluid: Vec::new(),",
        ["live-r48"],
        "the WINDOWED client meshes no waterlogged water while every unit test, "
        "every *shot gate and every other render-check witness stays green — "
        "the M86 shape, and the only reason r48 exists",
    ),
]


def run(cmd, timeout):
    try:
        p = subprocess.run(
            cmd,
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return None, ""
    return p.returncode, p.stdout + p.stderr


def build():
    code, _ = run(["cargo", "build", "-p", "rewo-app"], 900)
    return code == 0


CHECKERS = {
    "unit-mesh": lambda: run(["cargo", "test", "-q", "-p", "rewo-mesh", "--lib"], 600),
    "unit-data": lambda: run(["cargo", "test", "-q", "-p", "rewo-data", "--lib"], 600),
    "meshshot": lambda: run([EXE, "meshshot", "--check"], 300),
    "blockentityshot": lambda: run([EXE, "blockentityshot", "--check"], 600),
}


# The distinctive text of THIS branch's r48 row. Checked, not assumed — see
# `live_r48`.
R48_MARK = "carried-fluid cells in the largest column"


def live_r48():
    """The one per-witness verdict — see the module docstring.

    **Read from the run's OWN stdout first.** `tools/render_check.py` also
    writes `%TEMP%/rewo-render-check.out`, and that path is shared by every
    worktree on the machine: during M164b a concurrent wave agent's run
    overwrote it between two reads of the same file, and the `r48` row it then
    carried was a different branch's claim entirely (a mob nametag, not water).
    The stdout this function reads belongs to the process it just started, so
    it cannot be another branch's; the file is a fallback and is accepted only
    when the row still carries `R48_MARK`.
    """
    _code, out = run([sys.executable, "tools/render_check.py"], 900)
    rows = [ln for ln in out.splitlines() if " r48 " in ln and R48_MARK in ln]
    where = "stdout"
    if not rows:
        path = os.path.join(os.environ["TEMP"], "rewo-render-check.out")
        try:
            body = io.open(path, encoding="utf-8", errors="replace").read()
        except OSError:
            return 1, "no r48 row on stdout and no render-check output file"
        rows = [ln for ln in body.splitlines() if " r48 " in ln and R48_MARK in ln]
        where = "temp file"
    if len(rows) != 1:
        # Zero is the interesting case: either the run died before the rows, or
        # the only r48 present belongs to somebody else's branch.
        return 1, f"expected exactly one of THIS branch's r48 rows, found {len(rows)}"
    return (0 if "PASS r48" in rows[0] else 1), f"[{where}] " + rows[0][:110]


CHECKERS["live-r48"] = live_r48


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    body = MUTATIONS[lo:hi]
    # The control runs EVERY checker the slice will use, not just its own. A
    # control validated against `cargo test` says nothing about whether a
    # gate-routed or live-routed checker can return 0 at all — and a checker
    # that always returns nonzero reads as a clean sweep of KILLs.
    union, seen = [], set()
    for m in body:
        for c in m[4]:
            if c not in seen:
                seen.add(c)
                union.append(c)
    control = MUTATIONS[0][:4] + (union or MUTATIONS[0][4],) + MUTATIONS[0][5:]
    selected = [control] + body
    print(f"[m161] slice [{lo}, {hi}) — {len(body)} mutations + control ({', '.join(union)})")

    results = []
    for name, path, find, repl, checkers, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl)
            )
            if not build():
                survived, reason = False, "build failed"
            else:
                survived, reason = True, ""
                for c in checkers:
                    code, _out = CHECKERS[c]()
                    if code != 0:
                        survived = False
                        reason = f"{c} exit {code}"
                        break
                    reason = (reason + " " + f"{c} ok").strip()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), (
                f"RESTORE FAILED for {path} — mutation may be left on disk"
            )
            build()  # gate-routed: the restore does not rebuild by itself
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
        results.append((name, verdict, why))

    print()
    if not results or results[0][1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        return 2
    killed = sum(1 for _, v, _ in results[1:] if v == "KILLED")
    total = len(results) - 1
    print(f"control SURVIVED (battery is valid) - {killed}/{total} killed")
    for name, verdict, why in results[1:]:
        if verdict != "KILLED":
            print(f"  {verdict}: {name}\n    would mean: {why}")
    return 0 if killed == total else 1


if __name__ == "__main__":
    sys.exit(main())
