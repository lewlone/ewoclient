//! The M15 greedy-mesh oracle: an emitted-geometry decoder, the adversarial
//! fixtures and the release-compiled `check_greedy_oracle` gate behind
//! `rewo meshshot --check`. Compiled for tests and under the `oracle`
//! feature only, so a production client does not ship it.

#![allow(clippy::too_many_arguments)]

use super::*;

// -- M15 greedy oracle: emitted-geometry decoder ----------------------------
//
// A merged rectangle is deliberately *not* byte-comparable to the N unit quads
// it replaces, so diffing raw buffers against the reference could only ever say
// "different". The oracle therefore inverts the emitter: it re-derives each
// quad's face, plane, anchor block and extent purely from the emitted vertex
// positions, checks every corner against the generic scaling rule, and then
// enumerates the unit faces the rectangle covers. Comparing *that* expansion
// against `mesh_column_reference`'s is what proves the merge lost nothing.
//
// This lives in production (not `mod tests`) because `check_greedy_oracle` is a
// release-compiled gate. `mod tests` wraps these with `.expect(...)`, so both
// graders run the identical decode and cannot drift apart.

/// One unit face as both mesher paths must agree on it.
///
/// `light24` is the lower 24 bits (layer + block/sky light); the shade code
/// rides in `face` and AO is kept per corner, so nothing is smuggled through a
/// masked-off bit. Field order is the sort order — face, then block, then
/// appearance.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(crate) struct UnitFace {
    pub face: u8,
    pub block: [i32; 3],
    pub light24: u32,
    pub ao: [u8; 4],
    pub tint: u32,
}

/// A unit face plus the dimensions of the rectangle it was emitted inside.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExpandedFace {
    pub f: UnitFace,
    pub w: i32,
    pub h: i32,
}

/// Split a vertex/index stream into quads, checking the `[0,1,2,0,2,3]` pattern
/// every emitter in this file uses.
pub(crate) fn split_quads(v: &[MeshVertex], idx: &[u32]) -> Result<Vec<[MeshVertex; 4]>, String> {
    if v.len() % 4 != 0 {
        return Err(format!(
            "geometry is not whole quads: {} vertices is not a multiple of 4",
            v.len()
        ));
    }
    if idx.len() != v.len() / 4 * 6 {
        return Err(format!(
            "index count {} is not 6 per quad for {} vertices",
            idx.len(),
            v.len()
        ));
    }
    for q in 0..v.len() / 4 {
        let b = (q * 4) as u32;
        if idx[q * 6..q * 6 + 6] != [b, b + 1, b + 2, b, b + 2, b + 3] {
            return Err(format!(
                "quad {q} winding: indices {:?} are not [{b},{},{},{b},{},{}]",
                &idx[q * 6..q * 6 + 6],
                b + 1,
                b + 2,
                b + 2,
                b + 3
            ));
        }
    }
    Ok(v.chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect())
}

/// Expand cube geometry back into the unit faces it covers.
pub(crate) fn expand_unit_faces(
    v: &[MeshVertex],
    idx: &[u32],
) -> Result<Vec<ExpandedFace>, String> {
    expand_quad_list(&split_quads(v, idx)?)
}

/// [`expand_unit_faces`] over an already-selected quad list. A mixed column's
/// opaque stream also carries model and fluid quads, which are not axis-aligned
/// unit faces and must be filtered out before this runs.
pub(crate) fn expand_quad_list(qs: &[[MeshVertex; 4]]) -> Result<Vec<ExpandedFace>, String> {
    let mut out = Vec::new();
    for q in qs.iter().copied() {
        // The decoder recovers a quad's direction from its shade code, which is
        // only a bijection under `CardinalLighting::DEFAULT` (M16: the Nether
        // maps up *and* down onto code 6). The oracle column is a default-lit
        // world, so this holds there; anything else is refused rather than
        // indexed out of `FACE_AXES`.
        let face = q[0].shade_code() as usize;
        if face >= FACE_OFFSETS.len() {
            return Err(format!(
                "shade code {face} is not a face direction — unit-face expansion only decodes DEFAULT cardinal lighting"
            ));
        }
        let (au, av, _) = FACE_AXES[face];
        let an = 3 - au - av; // the remaining axis: 0+1+2 == 3
        let plane = q[0].pos[an];
        for vtx in &q {
            if vtx.pos[an] != plane {
                return Err(format!(
                    "face {face}: plane axis {an} is not constant ({} vs {plane})",
                    vtx.pos[an]
                ));
            }
        }
        let span = |axis: usize| {
            let lo = q.iter().map(|x| x.pos[axis]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|x| x.pos[axis]).fold(f32::MIN, f32::max);
            (lo, hi)
        };
        let (umin, umax) = span(au);
        let (vmin, vmax) = span(av);
        let (w, h) = ((umax - umin) as i32, (vmax - vmin) as i32);
        if w < 1 || h < 1 {
            return Err(format!("face {face}: degenerate rectangle {w}x{h}"));
        }

        // Anchor block: the face's own axis sits at `plane` minus whichever side
        // of the block the face is on (up/south/east are the +1 side).
        let mut base = [0i32; 3];
        base[au] = umin as i32;
        base[av] = vmin as i32;
        base[an] = (plane - FACE_CORNERS[face][0].0[an]) as i32;

        // Every corner must match the generic emitter rule exactly, and the UV
        // must scale with the extent (repeat, not stretch).
        for (i, (corner, uv)) in FACE_CORNERS[face].iter().enumerate() {
            let mut expect = [
                base[0] as f32 + corner[0],
                base[1] as f32 + corner[1],
                base[2] as f32 + corner[2],
            ];
            if corner[au] == 1.0 {
                expect[au] += (w - 1) as f32;
            }
            if corner[av] == 1.0 {
                expect[av] += (h - 1) as f32;
            }
            if q[i].pos != expect {
                return Err(format!(
                    "face {face} corner {i} position {:?} != expected {expect:?} for a {w}x{h} rectangle at {base:?}",
                    q[i].pos
                ));
            }
            let expect_uv = [uv[0] * w as f32, uv[1] * h as f32];
            if q[i].uv != expect_uv {
                return Err(format!(
                    "face {face} corner {i} uv {:?} != expected {expect_uv:?} (a rectangle must repeat its texture, not stretch it)",
                    q[i].uv
                ));
            }
        }

        let light24 = q[0].light & 0x00FF_FFFF;
        let tint = q[0].tint;
        for vtx in &q {
            if vtx.light & 0x00FF_FFFF != light24 {
                return Err(format!(
                    "face {face} at {base:?}: layer/light is not per-quad ({:#08x} vs {light24:#08x})",
                    vtx.light & 0x00FF_FFFF
                ));
            }
            if vtx.tint != tint {
                return Err(format!(
                    "face {face} at {base:?}: tint is not per-quad ({:#010x} vs {tint:#010x})",
                    vtx.tint
                ));
            }
        }
        let ao = [
            q[0].ao_code(),
            q[1].ao_code(),
            q[2].ao_code(),
            q[3].ao_code(),
        ];
        for dv in 0..h {
            for du in 0..w {
                let mut block = base;
                block[au] += du;
                block[av] += dv;
                out.push(ExpandedFace {
                    f: UnitFace {
                        face: face as u8,
                        block,
                        light24,
                        ao,
                        tint,
                    },
                    w,
                    h,
                });
            }
        }
    }
    Ok(out)
}

// -- M15 greedy oracle: the adversarial cube gate ---------------------------

/// Vanilla's spruce/birch `BlockTintSources.constant` colors — a real pair of
/// distinct constants, used here as the tint-boundary material.
pub(crate) const ORACLE_SPRUCE_RGB: [u8; 3] = [97, 153, 97];
pub(crate) const ORACLE_BIRCH_RGB: [u8; 3] = [128, 167, 85];

// Fixture block states (indices into `oracle_table`).
pub(crate) const OS_GRANITE: u32 = 1;
pub(crate) const OS_DIORITE: u32 = 2;
pub(crate) const OS_CUTOUT: u32 = 3;
pub(crate) const OS_SPRUCE: u32 = 4;
pub(crate) const OS_BIRCH: u32 = 5;

/// The atlas layer `GRANITE`, `DIORITE` and `CUTOUT` all carry on their **down**
/// face. Deliberately shared: it makes the block state the only thing that can
/// separate them in a down plane.
pub(crate) const OS_SHARED_DOWN_LAYER: u16 = 11;
/// The layer `SPRUCE`/`BIRCH` resolve to once the Constant tint engages (their
/// `raw_faces`). Observing this rather than `OS_TINT_LEGACY_LAYER` is what
/// proves the tint path actually ran.
pub(crate) const OS_TINT_RAW_LAYER: u16 = 51;
pub(crate) const OS_TINT_LEGACY_LAYER: u16 = 50;

/// The fixture's render table.
///
/// Every cube carries *distinct* layers per direction, so a merge that crossed a
/// face boundary would show up in the layer index. The three opaque materials
/// share exactly one layer — the down face — which is the material-boundary
/// probe. `CUTOUT` stands in for an alpha-tested block: `RenderKind::Cube` is
/// the pass's only cube representation (there is no cutout flag in the mesh
/// path), so a cutout block is a `Cube` with its own layers, and what the gate
/// grades is that it stays semantically present and never coalesces with a
/// neighbouring material.
pub(crate) fn oracle_table() -> Vec<RenderKind> {
    let opaque = |faces: [u16; 6]| RenderKind::Cube {
        faces,
        raw_faces: faces,
        tint: [TintSource::None; 6],
    };
    let tinted = |rgb: [u8; 3]| RenderKind::Cube {
        faces: [OS_TINT_LEGACY_LAYER; 6],
        raw_faces: [OS_TINT_RAW_LAYER; 6],
        tint: [TintSource::Constant(rgb); 6],
    };
    vec![
        RenderKind::Invisible,
        opaque([10, OS_SHARED_DOWN_LAYER, 12, 13, 14, 15]), // GRANITE
        opaque([20, OS_SHARED_DOWN_LAYER, 22, 23, 24, 25]), // DIORITE
        opaque([30, OS_SHARED_DOWN_LAYER, 32, 33, 34, 35]), // CUTOUT (alpha-test)
        tinted(ORACLE_SPRUCE_RGB),
        tinted(ORACLE_BIRCH_RGB),
    ]
}

/// A minimal biome context. `TintSource::Constant` short-circuits *after*
/// `biome_tint`'s `world.biome_context()?` gate, so without a context attached
/// the tint materials would silently fall back to the legacy pre-tinted layer
/// and the tint-boundary probe would be vacuous. Nothing here is consulted — a
/// constant tint reads no registry and no colormap — it only opens the gate.
pub(crate) fn oracle_biome_context() -> rewo_world::biome::BiomeContext {
    use rewo_world::biome::{BiomeContext, BiomeDef, BiomeRegistry, Colormaps, GrassModifier};
    let def = BiomeDef {
        music_volume: None,
        name: "rewo:oracle".into(),
        temperature: 0.5,
        downfall: 0.5,
        water_color: 0,
        grass_override: None,
        foliage_override: None,
        dry_foliage_override: None,
        grass_modifier: GrassModifier::None,
        sky_color: None,
        fog_color: None,
            has_precipitation: true,
        temperature_modifier: Default::default(),
        ambient_sounds: None,
        background_music: None,
    };
    BiomeContext::new(
        std::sync::Arc::new(BiomeRegistry::new(vec![def])),
        Colormaps::neutral(),
        0,
    )
}

/// The adversarial column — one column at (0,0) holding every property the gate
/// grades. Regions are separated by at least one block of air on the axis that
/// matters, so each probe is independent of the others (AO reaches one cell past
/// a face, so a one-block gap is enough).
///
/// - **A** `x1..6 z1..6 y60..69` GRANITE — the merge core. Its four walls are
///   6×10 and straddle the `y=64` section floor; its floor is a 6×6; its 36 tops
///   are the up carve-out.
/// - **B** `x1..6 z=10 y64..69` GRANITE + an occluder at `(2,65,9)` — the AO
///   discontinuity. The occluder sits diagonally off the wall's north side, so
///   it darkens single corners of nearby faces without culling them.
/// - **C/D** `x8..15 z1..9 y=62` — three z-bands of GRANITE / DIORITE / CUTOUT
///   sharing one down layer: the material + cutout boundary.
/// - **E** `x8..15 z11..14` at `y=66` and `y=70` — the light boundaries; the
///   pokes land on the *sampled neighbour* cells (`y=65` block, `y=69` sky),
///   because a face reads light from the cell it faces, not from its own.
/// - **F** `x1..8 z11..14 y=62` — SPRUCE | BIRCH, the constant-tint boundary.
pub(crate) fn oracle_world() -> World {
    use rewo_world::dimension::DimensionShape;
    use rewo_world::light::Channel;

    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    w.set_biome_context(std::sync::Arc::new(oracle_biome_context()));

    // A — the merge core, crossing the y=64 section floor.
    for x in 1..=6 {
        for z in 1..=6 {
            for y in 60..=69 {
                w.set_block(x, y, z, OS_GRANITE);
            }
        }
    }

    // B — the AO discontinuity wall + its diagonal occluder.
    for x in 1..=6 {
        for y in 64..=69 {
            w.set_block(x, y, 10, OS_GRANITE);
        }
    }
    w.set_block(2, 65, 9, OS_GRANITE);

    // C/D — three materials, one shared down layer, one plane.
    for x in 8..=15 {
        for z in 1..=9 {
            let state = match z {
                1..=3 => OS_GRANITE,
                4..=6 => OS_DIORITE,
                _ => OS_CUTOUT,
            };
            w.set_block(x, 62, z, state);
        }
    }

    // E — one plate per light channel.
    for x in 8..=15 {
        for z in 11..=14 {
            w.set_block(x, 66, z, OS_GRANITE); // grades block light at y=65
            w.set_block(x, 70, z, OS_GRANITE); // grades sky light at y=69
        }
    }

    // F — the constant-tint boundary.
    for x in 1..=8 {
        for z in 11..=14 {
            w.set_block(x, 62, z, if x <= 4 { OS_SPRUCE } else { OS_BIRCH });
        }
    }

    // The light discontinuities. `Column::empty_lit` already carries a full
    // sky array (every nibble 15) and no block array (reads 0), so poking one
    // channel in one region cannot disturb any other face's light.
    let shape = w.shape;
    if let Some(col) = w.column_mut(0, 0) {
        for x in 12..=15 {
            for z in 11..=14 {
                col.set_light(&shape, Channel::Block, x, 65, z, 12);
                col.set_light(&shape, Channel::Sky, x, 69, z, 7);
            }
        }
    }
    w
}

// -- M15 greedy oracle: the legacy (non-cube) controls ----------------------
//
// Only the cube path can merge. Models and fluids run the untouched
// `emit_model` / `emit_fluid` code, so for them the optimized mesher must be
// *byte-identical* to the reference — a strictly stronger claim than the cube
// path's semantic equality, and the one that proves the greedy pass did not
// perturb scan order, buffer interleaving or the opaque/translucent split.
//
// These are the constructions `mod tests` already uses, hoisted into production
// so the controls and the unit tests are provably the same fixture (the tests
// delegate to them). The combined water+lava test column is split into two
// single-fluid controls so each stream is graded on its own.

/// States: 0 air, 1 an opaque cube (unplaced here), 2 water, 3 lava.
pub(crate) fn oracle_fluid_table() -> Vec<RenderKind> {
    vec![
        RenderKind::Invisible,
        RenderKind::Cube {
            faces: [0; 6],
            raw_faces: [0; 6],
            tint: [TintSource::None; 6],
        },
        RenderKind::Fluid {
            layer: 1,
            raw_layer: 1,
            level: 0,
            lava: false,
        },
        RenderKind::Fluid {
            layer: 2,
            raw_layer: 2,
            level: 0,
            lava: true,
        },
    ]
}

pub(crate) fn oracle_model_table() -> Vec<RenderKind> {
    vec![RenderKind::Invisible, RenderKind::Model(0)]
}

/// Two quads: an unculled shaded one facing north, and a culled unshaded one
/// facing south — so the control covers the cull branch, the `shade: false`
/// branch and the raw/legacy layer split.
pub(crate) fn oracle_model_quads() -> Vec<Vec<rewo_data::assets::Quad>> {
    use rewo_data::assets::Quad;
    vec![vec![
        Quad {
            verts: [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 7,
            raw_layer: 8,
            cull: -1,
            dir: 2,
            tint: TintSource::None,
            shade: true,
            translucent: false,
        },
        Quad {
            verts: [
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            layer: 9,
            raw_layer: 9,
            cull: 3,
            dir: 3,
            tint: TintSource::None,
            shade: false,
            translucent: false,
        },
    ]]
}

/// A patch of models spanning the `y=64` section boundary, to exercise ordering.
pub(crate) fn oracle_model_world() -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    for x in 3..7 {
        for z in 3..7 {
            w.set_block(x, 63, z, 1);
            w.set_block(x, 64, z, 1);
        }
    }
    w
}

/// Water only — a two-deep pool, so the control covers the submerged-cell
/// full-height branch as well as the surface.
pub(crate) fn oracle_water_world() -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    for x in 3..7 {
        for z in 3..7 {
            w.set_block(x, 62, z, 2);
            w.set_block(x, 63, z, 2);
        }
    }
    w
}

/// Lava only — opaque and fullbright, so it lands in the *opaque* stream.
pub(crate) fn oracle_lava_world() -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    for x in 3..7 {
        for z in 3..7 {
            w.set_block(x + 8, 62, z, 3);
        }
    }
    w
}

/// Geometry a legacy (non-cube) control produced. Identical in both meshers by
/// construction, so one set of counts describes both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyControlCounts {
    pub opaque_vertices: usize,
    pub opaque_indices: usize,
    pub translucent_vertices: usize,
    pub translucent_indices: usize,
}

// -- M164: the waterlogged controls ----------------------------------------
//
// Face indices below are the MESHER's, except inside `CarriedFluid`, whose
// `self_occludes` is `rewo_data::assets::FACE_DIRS`-ordered (see
// `SELF_OCCLUDE_REMAP`). `DOWN` is `FACE_DIRS` index 2 and `UP` is 3.

/// `self_occludes` for a shape flush with the bottom of its block (a bottom
/// slab, a stair's lower step) — `FACE_DIRS` order.
pub(crate) const ORACLE_OCC_DOWN: u8 = 1 << 2;
/// `self_occludes` for a shape flush with the top of its block (a top slab).
pub(crate) const ORACLE_OCC_UP: u8 = 1 << 3;
/// All six — a `type=double` slab, and the ONLY shape in the fixture that
/// occludes a SIDE. Without it the side half of `shouldRenderFace` has no
/// witness at all, which is how M164's battery first found it SURVIVING.
///
/// Measured against the real bake, the 68 full-cube carriers are exactly the
/// waterlogged double slabs (`blockentityshot` names them) — but **that state
/// is command-only**, and an earlier version of this comment called it "the
/// common real case", which is backwards. `SlabBlock.getStateForPlacement:72`
/// writes `WATERLOGGED, false` the moment a slab becomes DOUBLE, and
/// `placeLiquid` / `canPlaceLiquid` (`:106-113`) both refuse a DOUBLE slab, so
/// `type=double, waterlogged=true` is reachable only through `/setblock`, a
/// structure or a datapack. It is a real state that vanilla renders this way,
/// so the fixture stays; what was wrong was the reason given for it. The side
/// rule's *ordinary* case is a waterlogged stair — 1,536 of the 2,560
/// waterlogged stair states carry a side bit, measured in `blockentityshot`.
pub(crate) const ORACLE_OCC_ALL: u8 = 0b11_1111;

/// A DRY `Model(0)`; the two waterlogged states below share its geometry.
pub(crate) const WL_DRY: u32 = 4;
/// The same model, waterlogged, with a bottom-slab occlusion shape.
pub(crate) const WL_BOTTOM_SLAB: u32 = 5;
/// The same model, waterlogged, with a top-slab occlusion shape.
pub(crate) const WL_TOP_SLAB: u32 = 6;
/// The same model, waterlogged, occluding on every face — a double slab.
pub(crate) const WL_DOUBLE_SLAB: u32 = 7;
/// A plain water source (`oracle_fluid_table`'s).
pub(crate) const WL_WATER: u32 = 2;

/// [`oracle_fluid_table`] (0 air, 1 an opaque cube, 2 water, **3 LAVA**) plus
/// three `Model(0)` states.
///
/// The three models share one model index deliberately: the only input that
/// differs between the dry control and the two waterlogged blocks is the
/// carried-fluid table, so a difference in the output cannot come from the
/// geometry. `oracle_waterlogged_states_are_what_they_are_named` pins the
/// indices — an earlier draft of this fixture counted the base table as three
/// entries rather than four and graded LAVA as its "dry model", and the only
/// thing that noticed was the opaque-byte-identity witness.
pub(crate) fn oracle_waterlogged_table() -> Vec<RenderKind> {
    let mut t = oracle_fluid_table();
    debug_assert_eq!(t.len(), WL_DRY as usize);
    for _ in WL_DRY..=WL_DOUBLE_SLAB {
        t.push(RenderKind::Model(0));
    }
    t
}

pub(crate) fn oracle_waterlogged_carried() -> Vec<Option<CarriedFluid>> {
    let wl = |self_occludes| {
        Some(CarriedFluid {
            layer: 1,
            raw_layer: 1,
            level: 0,
            falling: false,
            self_occludes,
        })
    };
    let mut c: Vec<Option<CarriedFluid>> = vec![None; WL_DOUBLE_SLAB as usize + 1];
    c[WL_BOTTOM_SLAB as usize] = wl(ORACLE_OCC_DOWN);
    c[WL_TOP_SLAB as usize] = wl(ORACLE_OCC_UP);
    c[WL_DOUBLE_SLAB as usize] = wl(ORACLE_OCC_ALL);
    c
}

/// One block of `state` in mid-air at (2, 62, 2) — nothing below, so the
/// fluid's down face is decided by the block's OWN occlusion shape alone.
pub(crate) fn oracle_waterlogged_world(state: u32) -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    w.set_block(2, 62, 2, state);
    w
}

/// A water source at (8, 62, 2) with `east` at (9, 62, 2) — the `same()`
/// witness. Everything else is air, so the only face in the plane x = 9 is the
/// pool's east side and (if `east` carries water) that block's west side.
pub(crate) fn oracle_pool_beside(east: u32) -> World {
    use rewo_world::dimension::DimensionShape;
    let mut w = World::new(DimensionShape::OVERWORLD);
    w.ensure_column(0, 0);
    w.set_block(8, 62, 2, WL_WATER);
    w.set_block(9, 62, 2, east);
    w
}

/// Classify a translucent stream's fluid quads by geometry alone — `(top, side,
/// bottom)`. Reads the emitted vertices, not the emitter: a bottom face has all
/// four vertices on the block floor, a top face none, a side face two.
pub(crate) fn fluid_face_counts(v: &[MeshVertex], floor: f32) -> (usize, usize, usize) {
    let (mut top, mut side, mut bottom) = (0, 0, 0);
    for q in v.chunks_exact(4) {
        match q.iter().filter(|x| (x.pos[1] - floor).abs() < 1e-6).count() {
            4 => bottom += 1,
            0 => top += 1,
            _ => side += 1,
        }
    }
    (top, side, bottom)
}

/// Quads whose four vertices all sit on the plane `x`.
pub(crate) fn quads_on_x_plane(v: &[MeshVertex], x: f32) -> usize {
    v.chunks_exact(4)
        .filter(|q| q.iter().all(|p| (p.pos[0] - x).abs() < 1e-6))
        .count()
}

/// What [`check_waterlogged`] measured. Every number is observed — the checker
/// returns `Err` before a report exists if any of them is wrong.
#[derive(Clone, Copy, Debug, Default)]
pub struct WaterloggedFaceCounts {
    /// `(top, side, bottom)` for the bottom-slab-shaped waterlogged block.
    pub bottom_slab: (usize, usize, usize),
    /// The same for the top-slab-shaped one.
    pub top_slab: (usize, usize, usize),
    /// And for the one that occludes on all six faces.
    pub double_slab: (usize, usize, usize),
    /// Translucent quads the DRY twin produced. Must be 0.
    pub dry_translucent_quads: usize,
    /// `ColumnMesh::carried_fluid_cells` for the bottom-slab world / the dry one.
    pub carried_cells: (u32, u32),
    /// Quads in the plane x = 9 with a waterlogged / a dry east neighbour.
    pub pool_plane_faces: (usize, usize),
}

/// Grade the M164 waterlogged path. Serverless, asset-free, and driven through
/// the production [`mesh_column`] — the fixture supplies only the two tables the
/// bake supplies in production.
pub(crate) fn check_waterlogged() -> Result<WaterloggedFaceCounts, String> {
    let table = oracle_waterlogged_table();
    let carried = oracle_waterlogged_carried();
    let models = oracle_model_quads();
    // The fixture's own indices, asserted before anything is graded against
    // them: `WL_DRY` must be a model that carries nothing, and the two
    // waterlogged states must be the SAME model that does. Without this the
    // checker happily grades whatever happens to sit at those indices.
    for (st, want_carried) in [
        (WL_DRY, false),
        (WL_BOTTOM_SLAB, true),
        (WL_TOP_SLAB, true),
        (WL_DOUBLE_SLAB, true),
    ] {
        if !matches!(table.get(st as usize), Some(RenderKind::Model(0))) {
            return Err(format!(
                "waterlogged fixture: state {st} is {:?}, not Model(0)",
                table.get(st as usize)
            ));
        }
        if carried.get(st as usize).copied().flatten().is_some() != want_carried {
            return Err(format!(
                "waterlogged fixture: state {st} carries {:?}, expected carried={want_carried}",
                carried.get(st as usize)
            ));
        }
    }
    let mesh = |state: u32| {
        mesh_column(
            &oracle_waterlogged_world(state),
            &table,
            &models,
            &carried,
            0,
            0,
        )
    };

    // 1. One state, two draws (`SectionCompiler.compile:89-97`). The dry twin is
    //    the same `RenderKind::Model` with the same model index, so the only
    //    input that differs is the carried-fluid table.
    let wl = mesh(WL_BOTTOM_SLAB).ok_or("waterlogged: the bottom-slab world meshed to nothing")?;
    let dry = mesh(WL_DRY).ok_or("waterlogged: the dry world meshed to nothing")?;
    if wl.vertices.is_empty() {
        return Err("waterlogged: the block's own model produced no opaque geometry".into());
    }
    if wl.tvertices.is_empty() {
        return Err(
            "waterlogged: the block carried water and produced NO translucent geometry — this is the whole feature"
                .into(),
        );
    }
    if !dry.tvertices.is_empty() {
        return Err(format!(
            "waterlogged: the DRY twin produced {} translucent vertices — the carried-fluid table, not the render kind, must decide",
            dry.tvertices.len()
        ));
    }
    if bytemuck::cast_slice::<_, u8>(&wl.vertices) != bytemuck::cast_slice::<_, u8>(&dry.vertices) {
        return Err(
            "waterlogged: the block's own opaque geometry changed when it carried water — the two draws must be independent"
                .into(),
        );
    }
    if (wl.carried_fluid_cells, dry.carried_fluid_cells) != (1, 0) {
        return Err(format!(
            "waterlogged: carried_fluid_cells is {} / {}, expected 1 / 0",
            wl.carried_fluid_cells, dry.carried_fluid_cells
        ));
    }

    // 2. `isFaceOccludedBySelf` on the down face, and its ABSENCE on the up one.
    //    The two worlds differ in exactly one bit of `self_occludes`.
    let top = mesh(WL_TOP_SLAB).ok_or("waterlogged: the top-slab world meshed to nothing")?;
    let a = fluid_face_counts(&wl.tvertices, 62.0);
    let b = fluid_face_counts(&top.tvertices, 62.0);
    if a != (1, 4, 0) {
        return Err(format!(
            "waterlogged: a block whose own DOWN face is covered emitted (top,side,bottom) {a:?}, expected (1,4,0) — `shouldRenderFace` (FluidRenderer:56-60) must suppress the down face"
        ));
    }
    if b != (1, 4, 1) {
        return Err(format!(
            "waterlogged: a block whose own UP face is covered emitted (top,side,bottom) {b:?}, expected (1,4,1) — `renderUp` (FluidRenderer:77) does NOT go through `shouldRenderFace`, so the top face survives, and the self mask must not touch the sides"
        ));
    }
    // The real case: a `type=double` slab occludes on every direction, so
    // `shouldRenderFace` suppresses all four sides AND the bottom, and the ONE
    // face vanilla still draws is the top — which is the whole point of
    // `renderUp` skipping the self test. The two witnesses above cannot see the
    // side half of the test at all (their masks name only up and down), which
    // M164's mutation battery found by watching a dropped side test survive.
    let d = mesh(WL_DOUBLE_SLAB).ok_or("waterlogged: the double-slab world meshed to nothing")?;
    let dbl = fluid_face_counts(&d.tvertices, 62.0);
    if dbl != (1, 0, 0) {
        return Err(format!(
            "waterlogged: a block that occludes on ALL SIX faces emitted (top,side,bottom) {dbl:?}, expected (1,0,0) — every face but the top goes through `shouldRenderFace`, and the top does not"
        ));
    }

    // 3. `isNeighborSameFluid` — an ordinary pool must stop drawing toward a
    //    waterlogged block, because that block's fluid type IS water.
    let near = |east: u32| {
        mesh_column(&oracle_pool_beside(east), &table, &models, &carried, 0, 0)
            .ok_or("waterlogged: a pool world meshed to nothing")
    };
    let toward_wl = quads_on_x_plane(&near(WL_BOTTOM_SLAB)?.tvertices, 9.0);
    let toward_dry = quads_on_x_plane(&near(WL_DRY)?.tvertices, 9.0);
    if (toward_wl, toward_dry) != (0, 1) {
        return Err(format!(
            "waterlogged: the plane x=9 holds {toward_wl} quads beside a waterlogged block and {toward_dry} beside a dry one, expected 0 and 1 — `same()` decides face suppression for ORDINARY pools too, so a version that only knows `RenderKind::Fluid` leaves a visible internal wall in every underwater build"
        ));
    }

    Ok(WaterloggedFaceCounts {
        bottom_slab: a,
        top_slab: b,
        double_slab: dbl,
        dry_translucent_quads: dry.tvertices.len() / 4,
        carried_cells: (wl.carried_fluid_cells, dry.carried_fluid_cells),
        pool_plane_faces: (toward_wl, toward_dry),
    })
}

/// Grade one legacy control: the optimized mesher must reproduce the reference
/// **byte for byte** on both streams, touch no cube metrics, and actually
/// produce the stream(s) the fixture exists to cover.
#[allow(clippy::too_many_arguments)]
pub(crate) fn oracle_check_legacy_control(
    world: &World,
    table: &[RenderKind],
    models: &[Vec<rewo_data::assets::Quad>],
    carried: &[Option<CarriedFluid>],
    what: &str,
    want_opaque: bool,
    want_translucent: bool,
) -> Result<LegacyControlCounts, String> {
    let r = mesh_column_reference(world, table, models, carried, 0, 0)
        .ok_or_else(|| format!("{what} control: the reference mesher produced nothing"))?;
    let o = mesh_column(world, table, models, carried, 0, 0)
        .ok_or_else(|| format!("{what} control: the optimized mesher produced nothing"))?;

    // Non-vacuity first: a control that emitted nothing would "match" trivially.
    if want_opaque && o.vertices.is_empty() {
        return Err(format!(
            "{what} control is vacuous: the opaque stream is empty, but this fixture exists to populate it"
        ));
    }
    if !want_opaque && !o.vertices.is_empty() {
        return Err(format!(
            "{what} control: {} vertices reached the opaque stream, which this fixture must leave empty",
            o.vertices.len()
        ));
    }
    if want_translucent && o.tvertices.is_empty() {
        return Err(format!(
            "{what} control is vacuous: the translucent stream is empty, but this fixture exists to populate it"
        ));
    }
    if !want_translucent && !o.tvertices.is_empty() {
        return Err(format!(
            "{what} control: {} vertices reached the translucent stream, which this fixture must leave empty",
            o.tvertices.len()
        ));
    }

    // Byte equality on both streams — models and fluids never merge, so nothing
    // about their geometry may change.
    let (ov, rv) = (
        bytemuck::cast_slice::<_, u8>(&o.vertices),
        bytemuck::cast_slice::<_, u8>(&r.vertices),
    );
    if ov != rv {
        let at = ov
            .iter()
            .zip(rv)
            .position(|(a, b)| a != b)
            .map(|i| format!("first differing byte at {i}"))
            .unwrap_or_else(|| format!("lengths {} vs {}", ov.len(), rv.len()));
        return Err(format!(
            "{what} control: opaque vertex bytes are not identical to the reference ({at}) — a non-cube path must pass through the greedy mesher untouched"
        ));
    }
    if o.indices != r.indices {
        return Err(format!(
            "{what} control: opaque indices differ from the reference ({} vs {} entries)",
            o.indices.len(),
            r.indices.len()
        ));
    }
    let (otv, rtv) = (
        bytemuck::cast_slice::<_, u8>(&o.tvertices),
        bytemuck::cast_slice::<_, u8>(&r.tvertices),
    );
    if otv != rtv {
        let at = otv
            .iter()
            .zip(rtv)
            .position(|(a, b)| a != b)
            .map(|i| format!("first differing byte at {i}"))
            .unwrap_or_else(|| format!("lengths {} vs {}", otv.len(), rtv.len()));
        return Err(format!(
            "{what} control: translucent vertex bytes are not identical to the reference ({at})"
        ));
    }
    if o.tindices != r.tindices {
        return Err(format!(
            "{what} control: translucent indices differ from the reference ({} vs {} entries)",
            o.tindices.len(),
            r.tindices.len()
        ));
    }
    if (o.y_min, o.y_max) != (r.y_min, r.y_max) {
        return Err(format!(
            "{what} control: vertical bounds differ ({}..{} vs {}..{})",
            o.y_min, o.y_max, r.y_min, r.y_max
        ));
    }

    // Nothing here is a cube, so every cube metric must be zero — otherwise the
    // greedy path is claiming faces it does not own.
    let metrics = (
        o.visible_cube_faces,
        o.greedy_candidate_faces,
        o.greedy_quads,
        o.unit_fallback_faces,
    );
    if metrics != (0, 0, 0, 0) {
        return Err(format!(
            "{what} control: cube metrics are {metrics:?}, expected all zero — models and fluids are not cube faces"
        ));
    }

    Ok(LegacyControlCounts {
        opaque_vertices: o.vertices.len(),
        opaque_indices: o.indices.len(),
        translucent_vertices: o.tvertices.len(),
        translucent_indices: o.tindices.len(),
    })
}

/// What [`check_greedy_oracle`] measured. Every number is observed, not
/// assumed — the checker returns `Err` before building one of these if any
/// property failed.
#[derive(Clone, Debug)]
pub struct GreedyOracleReport {
    /// Unit faces the frozen reference mesher emitted (one quad each).
    pub reference_unit_faces: usize,
    pub reference_quads: usize,
    /// Quads the optimized mesher emitted — for this cube-only fixture, every
    /// one is a cube rectangle (merged or 1×1 fallback).
    pub optimized_quads: usize,
    pub optimized_cube_rects: usize,
    pub vertex_reduction_percent: f32,
    pub index_reduction_percent: f32,
    pub visible_cube_faces: u32,
    pub greedy_candidate_faces: u32,
    pub greedy_quads: u32,
    pub unit_fallback_faces: u32,
    /// Largest rectangle area emitted per face direction, indexed by face
    /// (0=up, 1=down, 2=north, 3=south, 4=west, 5=east).
    pub max_rect_area_per_face: [i32; 6],
    pub up_faces: usize,
    pub max_up_rect_area: i32,
    /// Rectangles whose vertical span strictly crosses the `y=64` section floor.
    pub section_crossing_rects: usize,
    pub nonuniform_ao_faces: usize,
    /// The largest rectangle in the same plane as the AO fallbacks.
    pub ao_plane_max_rect_area: i32,
    /// The three shared-layer material strips, in scan order.
    pub material_boundary_rects: [(i32, i32); 3],
    pub material_boundary_layer: u16,
    /// Unit faces owned by the cutout material (identical in both meshers).
    pub cutout_faces: usize,
    pub block_light_boundary_rects: [(i32, i32); 2],
    pub sky_light_boundary_rects: [(i32, i32); 2],
    pub tint_boundary_rects: [(i32, i32); 2],
    pub tint_boundary_layer: u16,
    pub tint_boundary_words: [u32; 2],
    pub deterministic_runs: usize,

    // -- legacy (non-cube) controls: byte-identical to the reference ---------
    //
    // The three booleans are always `true` in a returned report — the checker
    // fails before constructing one otherwise. They exist so a caller printing
    // the report can state plainly that each control ran and matched, and the
    // counts show it was not vacuous.
    pub model_only_identical: bool,
    pub model_only: LegacyControlCounts,
    pub water_only_identical: bool,
    pub water_only: LegacyControlCounts,
    pub lava_only_identical: bool,
    pub lava_only: LegacyControlCounts,
    /// M164 — a waterlogged model, which produces BOTH streams from one state.
    pub waterlogged_identical: bool,
    pub waterlogged: LegacyControlCounts,
    pub waterlogged_faces: WaterloggedFaceCounts,
}

/// Locate the single expanded entry covering one unit face.
pub(crate) fn oracle_face_at(
    exp: &[ExpandedFace],
    face: u8,
    block: [i32; 3],
    what: &str,
) -> Result<ExpandedFace, String> {
    let hits: Vec<_> = exp
        .iter()
        .filter(|e| e.f.face == face && e.f.block == block)
        .collect();
    match hits.len() {
        1 => Ok(*hits[0]),
        n => Err(format!(
            "{what}: face {face} of {block:?} is covered {n} times, expected exactly once"
        )),
    }
}

pub(crate) fn oracle_rect_at(
    exp: &[ExpandedFace],
    face: u8,
    block: [i32; 3],
    what: &str,
) -> Result<(i32, i32), String> {
    oracle_face_at(exp, face, block, what).map(|e| (e.w, e.h))
}

/// Assert a rectangle's exact extent.
pub(crate) fn oracle_expect_rect(
    exp: &[ExpandedFace],
    face: u8,
    block: [i32; 3],
    want: (i32, i32),
    what: &str,
) -> Result<(i32, i32), String> {
    let got = oracle_rect_at(exp, face, block, what)?;
    if got != want {
        return Err(format!(
            "{what}: face {face} of {block:?} merged into a {}x{} rectangle, expected {}x{}",
            got.0, got.1, want.0, want.1
        ));
    }
    Ok(got)
}

/// **The M15 acceptance gate.** Grades the production [`mesh_column`] against
/// the frozen [`mesh_column_reference`] on one adversarial cube column.
///
/// The comparison is *semantic*, not byte-wise: the optimized rectangles are
/// expanded back into the unit faces they cover (see [`expand_unit_faces`]) and
/// that set — face direction, owning block, lower-24 layer/light, all four AO
/// codes, tint word — must equal the reference's exactly. A byte diff cannot
/// express this, because a merged rectangle is *supposed* to differ from the N
/// unit quads it replaces.
///
/// Serverless and asset-free: no jar, no network, no Vulkan. Compiled into
/// release builds on purpose — it is the gate, not a test fixture, and it never
/// calls the test harness (`mod tests` calls *it*).
///
/// Returns `Err` with a property-specific message on the first failure.
pub fn check_greedy_oracle() -> Result<GreedyOracleReport, String> {
    let world = oracle_world();
    let table = oracle_table();

    let r = mesh_column_reference(&world, &table, &[], &[], 0, 0)
        .ok_or("reference mesher produced nothing for the oracle column")?;
    let o = mesh_column(&world, &table, &[], &[], 0, 0)
        .ok_or("optimized mesher produced nothing for the oracle column")?;

    // The fixture is cube-only, so nothing may reach the translucent stream and
    // every opaque quad must be a cube rectangle.
    if !o.tvertices.is_empty() || !r.tvertices.is_empty() {
        return Err(format!(
            "cube-only fixture produced translucent geometry ({} optimized / {} reference vertices)",
            o.tvertices.len(),
            r.tvertices.len()
        ));
    }

    let exp = expand_unit_faces(&o.vertices, &o.indices)?;
    let exp_ref = expand_unit_faces(&r.vertices, &r.indices)?;

    // -- 1. metrics invariants ------------------------------------------------
    if o.visible_cube_faces != r.visible_cube_faces {
        return Err(format!(
            "visible cube faces disagree: optimized {} vs reference {} — the two scans no longer see the same surface",
            o.visible_cube_faces, r.visible_cube_faces
        ));
    }
    if o.visible_cube_faces != o.greedy_candidate_faces + o.unit_fallback_faces {
        return Err(format!(
            "metrics: {} visible cube faces != {} candidates + {} fallbacks",
            o.visible_cube_faces, o.greedy_candidate_faces, o.unit_fallback_faces
        ));
    }
    if o.greedy_quads > o.greedy_candidate_faces {
        return Err(format!(
            "metrics: {} rectangles from only {} candidates — a rectangle must consume at least one",
            o.greedy_quads, o.greedy_candidate_faces
        ));
    }
    if o.greedy_quads == 0 {
        return Err("metrics: the fixture merged nothing — greedy_quads is 0".into());
    }
    if o.unit_fallback_faces == 0 {
        return Err(
            "metrics: the fixture produced no fallback — up faces and AO discontinuities must fall back"
                .into(),
        );
    }
    let optimized_quads = o.vertices.len() / 4;
    if optimized_quads != (o.greedy_quads + o.unit_fallback_faces) as usize {
        return Err(format!(
            "metrics: {optimized_quads} emitted quads != {} rectangles + {} fallbacks (a model/fluid quad leaked into a cube-only fixture?)",
            o.greedy_quads, o.unit_fallback_faces
        ));
    }
    if exp.len() != o.visible_cube_faces as usize {
        return Err(format!(
            "the optimized output expands to {} unit faces but the scan saw {} visible cube faces",
            exp.len(),
            o.visible_cube_faces
        ));
    }

    // -- 2. the merge actually paid off --------------------------------------
    if o.vertices.len() >= r.vertices.len() || o.indices.len() >= r.indices.len() {
        return Err(format!(
            "no reduction: optimized {} verts / {} indices vs reference {} / {}",
            o.vertices.len(),
            r.vertices.len(),
            o.indices.len(),
            r.indices.len()
        ));
    }
    let pct = |opt: usize, refr: usize| (1.0 - opt as f32 / refr as f32) * 100.0;
    let vertex_reduction_percent = pct(o.vertices.len(), r.vertices.len());
    let index_reduction_percent = pct(o.indices.len(), r.indices.len());

    // -- 3. every enabled direction really merged ----------------------------
    let mut max_rect_area_per_face = [0i32; 6];
    for e in &exp {
        let slot = &mut max_rect_area_per_face[e.f.face as usize];
        *slot = (*slot).max(e.w * e.h);
    }
    const DIR_NAME: [&str; 6] = ["up", "down", "north", "south", "west", "east"];
    for face in 1..6 {
        if max_rect_area_per_face[face] <= 1 {
            return Err(format!(
                "direction {} ({face}) never merged: its largest rectangle covers {} unit face(s), expected >1",
                DIR_NAME[face], max_rect_area_per_face[face]
            ));
        }
    }
    // The five walls/floors the fixture is built to produce, pinned exactly.
    oracle_expect_rect(&exp, 1, [1, 60, 1], (6, 6), "region A floor")?;
    oracle_expect_rect(&exp, 2, [1, 60, 1], (6, 10), "region A north wall")?;
    oracle_expect_rect(&exp, 3, [1, 60, 6], (6, 10), "region A south wall")?;
    oracle_expect_rect(&exp, 4, [1, 60, 1], (6, 10), "region A west wall")?;
    oracle_expect_rect(&exp, 5, [6, 60, 1], (6, 10), "region A east wall")?;

    // -- 4. the up carve-out --------------------------------------------------
    let ups: Vec<_> = exp.iter().filter(|e| e.f.face == 0).collect();
    if ups.is_empty() {
        return Err("the fixture emitted no up faces — the up carve-out is untested".into());
    }
    for e in &ups {
        if e.w != 1 || e.h != 1 {
            return Err(format!(
                "up face of {:?} merged into a {}x{} rectangle — every up (+Y) face must stay a unit quad (see the merge site in mesh_column for the canonical-pixel measurement)",
                e.f.block, e.w, e.h
            ));
        }
    }

    // -- 5. rectangles cross the section floor -------------------------------
    let section_crossing_rects = split_quads(&o.vertices, &o.indices)?
        .into_iter()
        .filter(|q| {
            let lo = q.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
            let hi = q.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
            lo < 64.0 && hi > 64.0
        })
        .count();
    if section_crossing_rects == 0 {
        return Err(
            "no rectangle crossed the y=64 section floor — masks must span the column's whole occupied height"
                .into(),
        );
    }

    // -- 6. AO discontinuity falls back, its neighbours still merge -----------
    let mut nonuniform_ao_faces = 0usize;
    let mut nonuniform_vertical = 0usize;
    for e in &exp {
        if e.f.ao.iter().any(|a| *a != e.f.ao[0]) {
            nonuniform_ao_faces += 1;
            if e.f.face != 0 {
                nonuniform_vertical += 1;
            }
            if e.w != 1 || e.h != 1 {
                return Err(format!(
                    "face {} of {:?} has non-uniform corner AO {:?} yet merged into a {}x{} rectangle — a rectangle cannot reproduce a corner gradient",
                    e.f.face, e.f.block, e.f.ao, e.w, e.h
                ));
            }
        }
    }
    if nonuniform_vertical == 0 {
        // Two ways to land here, and the caller needs to tell them apart: either
        // the fixture stopped generating a discontinuity (the probe went vacuous
        // — up faces fall back regardless, so a top plane cannot grade this
        // rule), or the mesher merged the non-uniform faces and *flattened* their
        // gradient to one corner's code, which erases the evidence. The
        // surface-equality check below catches the second case explicitly.
        return Err(
            "no non-uniform-AO vertical face reached the output — either the fixture stopped producing a discontinuity, or the mesher merged one and flattened its corner gradient"
                .into(),
        );
    }
    // The fallback must be local to the discontinuity, not a plane-wide bail-out:
    // the far half of that same north plane is one rectangle.
    let ao_plane_max_rect_area = exp
        .iter()
        .filter(|e| e.f.face == 2 && e.f.block[2] == 10)
        .map(|e| e.w * e.h)
        .max()
        .unwrap_or(0);
    if ao_plane_max_rect_area <= 1 {
        return Err(format!(
            "the AO-discontinuity plane (north, z=10) merged nothing (largest rectangle covers {ao_plane_max_rect_area} unit face(s)) — the fallback must be local to the discontinuity, not plane-wide"
        ));
    }
    oracle_expect_rect(&exp, 2, [4, 64, 10], (3, 6), "AO plane, unoccluded half")?;

    // -- 7. material boundary with a shared layer ----------------------------
    let strips = [
        oracle_face_at(&exp, 1, [8, 62, 1], "material strip GRANITE")?,
        oracle_face_at(&exp, 1, [8, 62, 4], "material strip DIORITE")?,
        oracle_face_at(&exp, 1, [8, 62, 7], "material strip CUTOUT")?,
    ];
    let material_boundary_layer = (strips[0].f.light24 & 0xFFFF) as u16;
    if material_boundary_layer != OS_SHARED_DOWN_LAYER {
        return Err(format!(
            "material boundary probe is vacuous: the down layer is {material_boundary_layer}, expected the shared {OS_SHARED_DOWN_LAYER}"
        ));
    }
    for (i, s) in strips.iter().enumerate() {
        if s.f.light24 != strips[0].f.light24 || s.f.tint != strips[0].f.tint {
            return Err(format!(
                "material strip {i} differs in layer/light/tint ({:#08x}/{:#010x} vs {:#08x}/{:#010x}) — the probe must isolate the block state as the only discriminator",
                s.f.light24, s.f.tint, strips[0].f.light24, strips[0].f.tint
            ));
        }
        if s.f.ao != strips[0].f.ao {
            return Err(format!(
                "material strip {i} differs in AO {:?} vs {:?} — the probe must isolate the block state",
                s.f.ao, strips[0].f.ao
            ));
        }
        if (s.w, s.h) != (8, 3) {
            return Err(format!(
                "material strip {i} is a {}x{} rectangle, expected 8x3 — with an identical layer, light, tint and AO, the only thing that may keep these three states apart is the block state in the merge key",
                s.w, s.h
            ));
        }
    }

    // -- 8. the cutout material stays present and separate -------------------
    let is_cutout = |b: [i32; 3]| b[1] == 62 && (8..=15).contains(&b[0]) && (7..=9).contains(&b[2]);
    let cutout_faces = exp.iter().filter(|e| is_cutout(e.f.block)).count();
    let cutout_faces_ref = exp_ref.iter().filter(|e| is_cutout(e.f.block)).count();
    if cutout_faces == 0 {
        return Err("the cutout material emitted no faces — it must stay semantically present through the greedy path".into());
    }
    if cutout_faces != cutout_faces_ref {
        return Err(format!(
            "the cutout material lost faces: {cutout_faces} optimized vs {cutout_faces_ref} reference"
        ));
    }

    // -- 9. light discontinuities are not crossed ----------------------------
    let block_lo = oracle_face_at(&exp, 1, [8, 66, 11], "block-light plate, unlit half")?;
    let block_hi = oracle_face_at(&exp, 1, [12, 66, 11], "block-light plate, lit half")?;
    if block_lo.f.light24 == block_hi.f.light24 {
        return Err(format!(
            "block-light probe is vacuous: both halves carry light word {:#08x} — the poke did not reach the sampled neighbour cells at y=65",
            block_lo.f.light24
        ));
    }
    for (s, half) in [(block_lo, "unlit"), (block_hi, "lit")] {
        if (s.w, s.h) != (4, 4) {
            return Err(format!(
                "block-light {half} half is a {}x{} rectangle, expected 4x4 — a rectangle must not span a block-light discontinuity",
                s.w, s.h
            ));
        }
    }
    let sky_lo = oracle_face_at(&exp, 1, [8, 70, 11], "sky-light plate, bright half")?;
    let sky_hi = oracle_face_at(&exp, 1, [12, 70, 11], "sky-light plate, dim half")?;
    if sky_lo.f.light24 == sky_hi.f.light24 {
        return Err(format!(
            "sky-light probe is vacuous: both halves carry light word {:#08x} — the poke did not reach the sampled neighbour cells at y=69",
            sky_lo.f.light24
        ));
    }
    for (s, half) in [(sky_lo, "bright"), (sky_hi, "dim")] {
        if (s.w, s.h) != (4, 4) {
            return Err(format!(
                "sky-light {half} half is a {}x{} rectangle, expected 4x4 — a rectangle must not span a sky-light discontinuity",
                s.w, s.h
            ));
        }
    }

    // -- 10. the constant-tint boundary is not crossed -----------------------
    //
    // A tint is a property of the block state, so `TintSource::Constant` cannot
    // produce a *same-state* tint change — the two sides necessarily differ in
    // state too. What this grades is therefore the observable property (no
    // rectangle spans the tint change) plus the fact that the split cannot be
    // attributed to layer or light: both sides carry an identical `light24`.
    // Check 7 above separately proves the state field splits a shared layer.
    let spruce = oracle_face_at(&exp, 1, [1, 62, 11], "tint boundary, spruce half")?;
    let birch = oracle_face_at(&exp, 1, [5, 62, 11], "tint boundary, birch half")?;
    let tint_boundary_layer = (spruce.f.light24 & 0xFFFF) as u16;
    if tint_boundary_layer != OS_TINT_RAW_LAYER {
        return Err(format!(
            "tint boundary probe is vacuous: the face resolved to layer {tint_boundary_layer}, not the raw {OS_TINT_RAW_LAYER} — the Constant tint path did not engage (is the biome context attached?)"
        ));
    }
    if spruce.f.light24 != birch.f.light24 {
        return Err(format!(
            "tint boundary probe is impure: the halves also differ in layer/light ({:#08x} vs {:#08x}), so a split would not prove the tint was honoured",
            spruce.f.light24, birch.f.light24
        ));
    }
    if spruce.f.tint == birch.f.tint {
        return Err(format!(
            "tint boundary probe is vacuous: both halves carry tint word {:#010x}",
            spruce.f.tint
        ));
    }
    if spruce.f.tint != pack_tint(ORACLE_SPRUCE_RGB, 0)
        || birch.f.tint != pack_tint(ORACLE_BIRCH_RGB, 0)
    {
        return Err(format!(
            "tint boundary: the emitted tint words {:#010x}/{:#010x} are not the fixture's constants {:#010x}/{:#010x} — the constant tint is not carried losslessly",
            spruce.f.tint,
            birch.f.tint,
            pack_tint(ORACLE_SPRUCE_RGB, 0),
            pack_tint(ORACLE_BIRCH_RGB, 0)
        ));
    }
    for (s, half) in [(spruce, "spruce"), (birch, "birch")] {
        if (s.w, s.h) != (4, 4) {
            return Err(format!(
                "tint boundary {half} half is a {}x{} rectangle, expected 4x4 — a rectangle must not span a tint change",
                s.w, s.h
            ));
        }
    }

    // -- 11. the whole surface, face for face --------------------------------
    let mut got: Vec<UnitFace> = exp.iter().map(|e| e.f).collect();
    let mut want: Vec<UnitFace> = exp_ref.iter().map(|e| e.f).collect();
    got.sort();
    want.sort();
    if got.len() != want.len() {
        return Err(format!(
            "expanded surface size differs: optimized {} unit faces vs reference {}",
            got.len(),
            want.len()
        ));
    }
    if let Some((i, (g, w))) = got.iter().zip(&want).enumerate().find(|(_, (g, w))| g != w) {
        return Err(format!(
            "expanded surface differs at sorted face {i}: optimized {g:?} vs reference {w:?}"
        ));
    }

    // -- 12. determinism ------------------------------------------------------
    let deterministic_runs = 4;
    for run in 0..deterministic_runs {
        let again = mesh_column(&world, &table, &[], &[], 0, 0)
            .ok_or("determinism: a rerun produced nothing")?;
        if bytemuck::cast_slice::<_, u8>(&again.vertices)
            != bytemuck::cast_slice::<_, u8>(&o.vertices)
        {
            return Err(format!(
                "determinism: rerun {run} emitted different vertex bytes — the masks must be index-ordered arrays, not hash maps"
            ));
        }
        if again.indices != o.indices {
            return Err(format!(
                "determinism: rerun {run} emitted different indices"
            ));
        }
        if (again.greedy_quads, again.unit_fallback_faces)
            != (o.greedy_quads, o.unit_fallback_faces)
        {
            return Err(format!(
                "determinism: rerun {run} reported {}/{} rectangles/fallbacks vs {}/{}",
                again.greedy_quads,
                again.unit_fallback_faces,
                o.greedy_quads,
                o.unit_fallback_faces
            ));
        }
    }

    // -- 13. legacy controls: models and fluids pass through untouched -------
    //
    // The cube path is graded semantically because a rectangle is *supposed* to
    // differ from the faces it replaces. Models and fluids merge nothing, so
    // they are held to byte equality — which is what proves the greedy pass
    // left scan order, buffer layout and the opaque/translucent split alone.
    let model_only = oracle_check_legacy_control(
        &oracle_model_world(),
        &oracle_model_table(),
        &oracle_model_quads(),
        &[],
        "model-only",
        true,  // models land in the opaque stream
        false, // and never in the translucent one
    )?;
    let fluid_table = oracle_fluid_table();
    let water_only = oracle_check_legacy_control(
        &oracle_water_world(),
        &fluid_table,
        &[],
        &[],
        "water-only",
        false, // water blends, so the opaque stream must stay empty
        true,
    )?;
    let lava_only = oracle_check_legacy_control(
        &oracle_lava_world(),
        &fluid_table,
        &[],
        &[],
        "lava-only",
        true, // lava is opaque and fullbright
        false,
    )?;
    // M164 — one waterlogged block. The only control that must populate BOTH
    // streams, because it is the only fixture where one block state runs both
    // of `SectionCompiler`'s draws.
    let waterlogged = oracle_check_legacy_control(
        &oracle_waterlogged_world(WL_BOTTOM_SLAB),
        &oracle_waterlogged_table(),
        &oracle_model_quads(),
        &oracle_waterlogged_carried(),
        "waterlogged",
        true, // the block's own model
        true, // and the water it carries
    )?;
    let waterlogged_faces = check_waterlogged()?;

    Ok(GreedyOracleReport {
        reference_unit_faces: exp_ref.len(),
        reference_quads: r.vertices.len() / 4,
        optimized_quads,
        optimized_cube_rects: optimized_quads,
        vertex_reduction_percent,
        index_reduction_percent,
        visible_cube_faces: o.visible_cube_faces,
        greedy_candidate_faces: o.greedy_candidate_faces,
        greedy_quads: o.greedy_quads,
        unit_fallback_faces: o.unit_fallback_faces,
        max_rect_area_per_face,
        up_faces: ups.len(),
        max_up_rect_area: max_rect_area_per_face[0],
        section_crossing_rects,
        nonuniform_ao_faces,
        ao_plane_max_rect_area,
        material_boundary_rects: [
            (strips[0].w, strips[0].h),
            (strips[1].w, strips[1].h),
            (strips[2].w, strips[2].h),
        ],
        material_boundary_layer,
        cutout_faces,
        block_light_boundary_rects: [(block_lo.w, block_lo.h), (block_hi.w, block_hi.h)],
        sky_light_boundary_rects: [(sky_lo.w, sky_lo.h), (sky_hi.w, sky_hi.h)],
        tint_boundary_rects: [(spruce.w, spruce.h), (birch.w, birch.h)],
        tint_boundary_layer,
        tint_boundary_words: [spruce.f.tint, birch.f.tint],
        deterministic_runs,
        model_only_identical: true,
        model_only,
        water_only_identical: true,
        water_only,
        lava_only_identical: true,
        lava_only,
        waterlogged_identical: true,
        waterlogged,
        waterlogged_faces,
    })
}
