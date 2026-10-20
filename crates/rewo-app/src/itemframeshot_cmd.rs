//! `rewo itemframeshot --check` — the item frame gate (M194).
//!
//! Item frames draw through the production chain end to end: the frame's
//! four block models baked from the jar (`block/item_frame` and kin), the
//! entity table's frame state read by `live_cmd::collect_frame_inputs`, the
//! draws `live_cmd::frame_draws` builds from it, the entity pass's map band
//! (`prepare_frame_maps`) and the emitter. Three groups of witnesses:
//!
//! - **b** — the bake, against the jar's own JSON (element counts and bounds
//!   of `template_item_frame*`, the FIXED display transforms of
//!   `item/generated` and `block/block`);
//! - **d** — the collector and the draw builder, against the decompile
//!   (`ItemFrameRenderer.extractRenderState` / `submit`: which model, map
//!   before item, the glow frame's light constants, invisibility);
//! - **p** — pixels, at positions derived here by hand from
//!   `ItemFrameRenderer.submit`'s pose chain and `MapRenderer.render`, never
//!   from `rewo_gpu::item_frame` (the poses under test). Colours are judged
//!   against the jar's PNG texels by chromaticity.
//!
//! **Fail-closed** on [`EXPECTED_WITNESSES`]: every witness records
//! unconditionally, and a count that is not exactly the pinned one fails.

use std::path::{Path, PathBuf};

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};

use rewo_data::{assets, DataPaths};
use rewo_gpu::entities::FrameMapDecoration;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_world::entities::{EntityState, FrameItem};
use rewo_world::maps::{MapDecoration, MapItemData, MapPatch, MapStore};

use crate::live_cmd::{
    collect_frame_inputs, frame_draws, prepare_frame_maps, FrameInputs, FramedMap, ItemFrameDraws,
};
use crate::stats::OverlayRing;

/// b1-b5, d1-d7, p1-p11.
const EXPECTED_WITNESSES: usize = 23;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 512;
const H: u32 = 512;
/// The camera's distance from the frame, along its facing.
const DIST: f32 = 1.5;

#[derive(ClapArgs)]
pub struct ItemframeshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Assert the item frame properties and exit nonzero on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Optional: also dump the rendered frames here (eyeball artifact).
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Bypass Vulkan validation layers.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[itemframeshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: ItemframeshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let etypes = rewo_data::entity_types::EntityTypes::load(&paths.registries_json())?;
    let items = rewo_data::items::Items::load(&paths.registries_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_bake(&mut c, &jar, &baked)?;
    check_draws(&mut c, &etypes, &items)?;

    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[itemframeshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "itemframeshot check: Vulkan validation requested but not active — \
            install the Vulkan SDK, or pass --no-validation"
                .into(),
        );
    }
    check_pixels(&mut c, &mut gpu, &jar, &baked, &etypes, &items, &args)?;

    println!(
        "[itemframeshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "itemframeshot: {} witnesses failed: {}",
            c.failures.len(),
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "itemframeshot: witness count {} != expected {EXPECTED_WITNESSES} — a named \
             property was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[itemframeshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// b — the bake, against the jar.
// ---------------------------------------------------------------------------

/// `(faces, min corner, max corner)` of a block model's `elements`, read
/// straight from the jar's JSON, in model units.
fn jar_elements(jar: &Path, model: &str) -> Option<(usize, [f32; 3], [f32; 3])> {
    let text = assets::jar_text(jar, &format!("assets/minecraft/models/block/{model}.json"))?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut faces = 0;
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for e in v.get("elements")?.as_array()? {
        faces += e.get("faces")?.as_object()?.len();
        let from = e.get("from")?.as_array()?;
        let to = e.get("to")?.as_array()?;
        for k in 0..3 {
            let (a, b) = (from[k].as_f64()? as f32, to[k].as_f64()? as f32);
            lo[k] = lo[k].min(a.min(b));
            hi[k] = hi[k].max(a.max(b));
        }
    }
    Some((faces, lo, hi))
}

/// A `display.<context>` entry from the jar, through vanilla's deserializer
/// (translation x 0.0625).
fn jar_display(jar: &Path, model: &str, context: &str) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
    let text = assets::jar_text(jar, &format!("assets/minecraft/models/{model}.json"))?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let d = v.get("display")?.get(context)?;
    let arr = |key: &str, default: f32| -> [f32; 3] {
        d.get(key)
            .and_then(|a| a.as_array())
            .map(|a| std::array::from_fn(|i| a[i].as_f64().unwrap_or(default as f64) as f32))
            .unwrap_or([default; 3])
    };
    let t = arr("translation", 0.0);
    Some((
        arr("rotation", 0.0),
        [t[0] * 0.0625, t[1] * 0.0625, t[2] * 0.0625],
        arr("scale", 1.0),
    ))
}

fn check_bake(c: &mut Checker, jar: &Path, baked: &assets::BakedAssets) -> Result<(), String> {
    use rewo_data::block_entity_models::ITEM_FRAME_MODELS;
    let held = &baked.held_items;
    let baked_models: Vec<&str> = ITEM_FRAME_MODELS
        .iter()
        .map(|(_, n)| *n)
        .filter(|n| held.block_entities.contains_key(*n))
        .collect();
    c.record(
        "b1.the_four_frame_models_bake",
        baked_models.len() == 4,
        format!("{} of 4 baked: {baked_models:?}", baked_models.len()),
    );

    // Each model against its template's elements: one quad per face, and the
    // same bounds (model units).
    let mut rows = Vec::new();
    let mut ok = true;
    for (block, name) in ITEM_FRAME_MODELS {
        let template = if block.ends_with("_map") {
            "template_item_frame_map"
        } else {
            "template_item_frame"
        };
        let want = jar_elements(jar, template);
        let got = held.block_entities.get(name).map(|m| {
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for q in &m.quads {
                for v in &q.verts {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                }
            }
            (m.quads.len(), lo, hi)
        });
        let same = match (want, got) {
            (Some((wf, wl, wh)), Some((gf, gl, gh))) => {
                wf == gf
                    && (0..3).all(|k| (wl[k] - gl[k]).abs() < 1e-3 && (wh[k] - gh[k]).abs() < 1e-3)
            }
            _ => false,
        };
        ok &= same;
        rows.push(format!("{name}: jar {want:?} baked {got:?}"));
    }
    c.record(
        "b2.each_model_is_its_templates_elements",
        ok,
        rows.join("; "),
    );

    // The glow frame is the plain frame with another back sprite: same
    // geometry, and the textures differ on exactly the back quads.
    let plain = held.block_entities.get("rewo:be/item_frame");
    let glow = held.block_entities.get("rewo:be/glow_item_frame");
    let (same_geo, differing) = match (plain, glow) {
        (Some(p), Some(g)) if p.quads.len() == g.quads.len() => (
            p.quads
                .iter()
                .zip(&g.quads)
                .all(|(a, b)| a.verts == b.verts),
            p.quads
                .iter()
                .zip(&g.quads)
                .filter(|(a, b)| a.tex != b.tex)
                .count(),
        ),
        _ => (false, 0),
    };
    // `template_item_frame`'s first element carries `#back` on two faces.
    c.record(
        "b3.the_glow_frame_differs_only_in_its_back_sprite",
        same_geo && differing == 2,
        format!("same geometry {same_geo}, {differing} quads with another texture (want 2)"),
    );

    // FIXED, against the jar: `item/generated` for a sprite, `block/block` for
    // a block item.
    let sword = held.models.get("minecraft:diamond_sword").map(|m| m.fixed);
    let want_sword = jar_display(jar, "item/generated", "fixed");
    c.record(
        "b4.a_sprite_items_fixed_is_item_generateds",
        match (sword, want_sword) {
            (Some(s), Some((r, t, k))) => s.rotation == r && s.translation == t && s.scale == k,
            _ => false,
        },
        format!("baked {sword:?}, jar {want_sword:?}"),
    );
    let dirt = held.models.get("minecraft:dirt").map(|m| m.fixed);
    let want_dirt = jar_display(jar, "block/block", "fixed");
    c.record(
        "b5.a_block_items_fixed_is_block_blocks",
        match (dirt, want_dirt) {
            (Some(s), Some((r, t, k))) => s.rotation == r && s.translation == t && s.scale == k,
            _ => false,
        },
        format!("baked {dirt:?}, jar {want_dirt:?}"),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// d — the collector and the draw builder.
// ---------------------------------------------------------------------------

/// A world holding the given entities; `(id, type, pos, spawn data)`.
fn world_with(ents: &[(i32, i32, [f64; 3], i32)]) -> rewo_world::World {
    let mut world = rewo_world::World::new(rewo_world::dimension::DimensionShape::OVERWORLD);
    for &(id, ty, p, data) in ents {
        let mut e = EntityState::new(id as u128, ty, p[0], p[1], p[2], 0.0, 0.0);
        e.spawn_data = data;
        world.entities.add(id, e);
    }
    world
}

fn frame_item(
    items: &rewo_data::items::Items,
    name: &str,
    map_id: Option<i32>,
) -> Option<FrameItem> {
    Some(FrameItem {
        item_id: items.id(name)?,
        map_id,
        foil: false,
    })
}

/// A map store holding `id` with the quadrant pattern and two decorations: a
/// frame marker (`showOnItemFrame`) top-left and a player arrow (not) top-right.
fn quadrant_map(id: i32) -> MapStore {
    let mut colors = vec![0u8; 128 * 128];
    for v in 0..128usize {
        for u in 0..128usize {
            colors[u + v * 128] = match (u < 64, v < 64) {
                (true, true) => QUAD_PACKED[0],
                (false, true) => QUAD_PACKED[1],
                (true, false) => QUAD_PACKED[2],
                (false, false) => QUAD_PACKED[3],
            };
        }
    }
    let mut store = MapStore::default();
    store.apply(&MapItemData {
        map_id: id,
        scale: 0,
        locked: false,
        decorations: Some(vec![
            // `frame` (raw id 1, showOnItemFrame true) and `player` (0, false).
            MapDecoration {
                type_id: 1,
                x: -64,
                y: -64,
                rot: 0,
                name: None,
            },
            MapDecoration {
                type_id: 0,
                x: 64,
                y: -64,
                rot: 0,
                name: None,
            },
        ]),
        patch: Some(MapPatch {
            start_x: 0,
            start_y: 0,
            width: 128,
            height: 128,
            colors,
        }),
    });
    store
}

/// Packed `MapColor` ids at brightness HIGH (`id << 2 | 2`, the colour
/// unscaled): FIRE red top-left, WATER blue top-right, PLANT green
/// bottom-left, COLOR_YELLOW bottom-right.
const QUAD_PACKED: [u8; 4] = [4 << 2 | 2, 12 << 2 | 2, 7 << 2 | 2, 18 << 2 | 2];

fn check_draws(
    c: &mut Checker,
    etypes: &rewo_data::entity_types::EntityTypes,
    items: &rewo_data::items::Items,
) -> Result<(), String> {
    let frame = etypes
        .id_of("minecraft:item_frame")
        .ok_or("no item_frame type")?;
    let glow = etypes
        .id_of("minecraft:glow_item_frame")
        .ok_or("no glow_item_frame type")?;
    let zombie = etypes.id_of("minecraft:zombie").ok_or("no zombie type")?;
    let lm = rewo_world::lightmap::LightmapState::default();

    // d1/d2: a north frame with a sword turned once by metadata; a glow frame
    // whose facing (up) arrived only in the spawn packet; a zombie.
    let mut world = world_with(&[
        (1, frame, [0.0, 64.0, 0.0], 3),
        (2, glow, [4.0, 64.0, 0.0], 1),
        (3, zombie, [8.0, 64.0, 0.0], 0),
    ]);
    world.entities.set_frame_facing(1, 2);
    world
        .entities
        .set_frame_item(1, frame_item(items, "minecraft:diamond_sword", None));
    world.entities.set_frame_rotation(1, 9);
    let maps = MapStore::default();
    let inputs = collect_frame_inputs(&world, &maps, None, etypes, items, 1.0);
    let first = inputs.first();
    c.record(
        "d1.a_frame_reads_its_facing_item_and_rotation",
        inputs.len() == 2
            && first.is_some_and(|f| {
                f.facing == 2
                    && f.rotation == 1
                    && !f.glow
                    && f.item.as_deref() == Some("minecraft:diamond_sword")
                    && f.map.is_none()
            }),
        format!(
            "{} frames; first {first:?} — slot 8 beats spawn data, rotation 9 % 8 = 1",
            inputs.len()
        ),
    );
    let second = inputs.get(1);
    c.record(
        "d2.the_spawn_packets_facing_stands_and_only_frames_are_collected",
        second.is_some_and(|f| f.facing == 1 && f.glow && f.item.is_none()),
        format!("second {second:?} — the zombie is not a frame"),
    );

    // d3/d4: a map with data draws the map (and only the frame's decorations);
    // a map the level does not hold is drawn as the item.
    let mut world = world_with(&[
        (1, frame, [0.0, 64.0, 0.0], 2),
        (2, frame, [2.0, 64.0, 0.0], 2),
    ]);
    world
        .entities
        .set_frame_item(1, frame_item(items, "minecraft:filled_map", Some(7)));
    world
        .entities
        .set_frame_item(2, frame_item(items, "minecraft:filled_map", Some(8)));
    let maps = quadrant_map(7);
    let inputs = collect_frame_inputs(&world, &maps, None, etypes, items, 1.0);
    let with_data = inputs.first().and_then(|f| f.map.as_ref());
    c.record(
        "d3.a_map_with_data_keeps_only_its_frame_decorations",
        with_data.is_some_and(|m| {
            m.id == 7
                && m.decorations
                    == [FrameMapDecoration {
                        type_id: 1,
                        x: -64,
                        y: -64,
                        rot: 0,
                    }]
        }),
        format!("{with_data:?} — `player` is not showOnItemFrame, `frame` is"),
    );
    let mut out = ItemFrameDraws::default();
    for f in &inputs {
        frame_draws(f, &lm, &mut out);
    }
    let names: Vec<&str> = out.models.iter().map(|m| m.model.as_str()).collect();
    c.record(
        "d4.map_before_item_and_the_map_frame_model",
        names
            == [
                "rewo:be/item_frame_map",
                "rewo:be/item_frame",
                "minecraft:filled_map",
            ]
            && out.maps.len() == 1
            && out.maps[0].map_id == 7,
        format!(
            "models {names:?}, maps {:?} — `state.mapId != null` only when the level \
             holds the data",
            out.maps.iter().map(|m| m.map_id).collect::<Vec<_>>()
        ),
    );

    // d5: an invisible frame draws no frame model and puts its item flush
    // with the wall: `translate(0, 0, 0.5)` instead of 0.4375, so the item's
    // centre is `step * (0.46875 - 0.5)` from the entity.
    let inv = FrameInputs {
        pos: [0.0; 3],
        facing: 5,
        rotation: 0,
        invisible: true,
        glow: false,
        item: Some("minecraft:diamond_sword".into()),
        foil: false,
        map: None,
        light: (15, 15),
        profile: None,
    };
    let mut out = ItemFrameDraws::default();
    frame_draws(&inv, &lm, &mut out);
    let item = out.models.first();
    let centre = item.map(|d| [d.transform[0][3], d.transform[1][3], d.transform[2][3]]);
    c.record(
        "d5.an_invisible_frame_has_no_model_and_a_flush_item",
        out.models.len() == 1
            && item.is_some_and(|d| d.fixed_display)
            && centre.is_some_and(|p| (p[0] - (0.46875 - 0.5)).abs() < 1e-6 && p[1].abs() < 1e-6),
        format!(
            "{} draws, item centre {centre:?} (want [-0.03125, 0, 0] for east)",
            out.models.len()
        ),
    );

    // d6/d7: a glow frame in the dark. The frame takes block light >= 5
    // (`getBlockLightLevel`), the item 15728880 (full bright), the map
    // 15728850; a plain frame's item takes the frame's own light.
    let dark = |glow: bool, map: Option<FramedMap>| FrameInputs {
        pos: [0.0; 3],
        facing: 2,
        rotation: 0,
        invisible: false,
        glow,
        item: Some("minecraft:diamond_sword".into()),
        foil: false,
        map,
        light: (0, 0),
        profile: None,
    };
    // The lightmap's `0/0` black texel is NaN, which every entity light
    // clamps to 0 (`live_cmd::entity_light`'s guard), so the expectation does
    // too.
    let s =
        |b, k| rewo_world::lightmap::sample(b, k, &lm).map(|v| if v.is_finite() { v } else { 0.0 });
    let mut g = ItemFrameDraws::default();
    frame_draws(&dark(true, None), &lm, &mut g);
    let mut p = ItemFrameDraws::default();
    frame_draws(&dark(false, None), &lm, &mut p);
    let lights = |o: &ItemFrameDraws| o.models.iter().map(|m| m.light).collect::<Vec<_>>();
    c.record(
        "d6.a_glow_frames_model_and_item_lights",
        lights(&g) == [s(5, 0), s(15, 15)] && lights(&p) == [s(0, 0), s(0, 0)],
        format!(
            "glow {:?}, plain {:?} — want [s(5,0), s(15,15)] and [s(0,0), s(0,0)]",
            lights(&g),
            lights(&p)
        ),
    );
    let mut gm = ItemFrameDraws::default();
    frame_draws(
        &dark(
            true,
            Some(FramedMap {
                id: 1,
                version: 1,
                decorations: Vec::new(),
                labels: Vec::new(),
            }),
        ),
        &lm,
        &mut gm,
    );
    c.record(
        "d7.a_glow_frames_map_light",
        gm.maps.len() == 1
            && gm.maps[0].light == s(13, 15)
            && gm.models.len() == 1
            && gm.models[0].model == "rewo:be/glow_item_frame_map",
        format!(
            "maps {:?}, models {:?} — 15728850 is sky 15, block coordinate 210",
            gm.maps.iter().map(|m| m.light).collect::<Vec<_>>(),
            gm.models.iter().map(|m| &m.model).collect::<Vec<_>>()
        ),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// p — pixels.
// ---------------------------------------------------------------------------

/// `Direction.from3DDataValue(facing).getUnitVec3()`, transcribed again here
/// rather than borrowed from the module under test.
fn step_of(facing: u8) -> Vec3 {
    match facing {
        0 => Vec3::NEG_Y,
        1 => Vec3::Y,
        2 => Vec3::NEG_Z,
        3 => Vec3::Z,
        4 => Vec3::NEG_X,
        _ => Vec3::X,
    }
}

/// Two axes in the frame's plane (the border is square, so any orthonormal
/// pair will do) and the camera's up.
fn plane_axes(facing: u8) -> (Vec3, Vec3) {
    let s = step_of(facing);
    let a = if s.y.abs() > 0.5 { Vec3::X } else { Vec3::Y };
    (a, s.cross(a))
}

fn view_proj(facing: u8) -> Mat4 {
    let s = step_of(facing);
    let up = if s.y.abs() > 0.5 { Vec3::Z } else { Vec3::Y };
    let eye = s * DIST;
    let view = crate::view_math::look_to_rh(eye, -s, up);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        40f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    proj * view
}

fn project(vp: &Mat4, p: Vec3) -> (u32, u32) {
    let c = *vp * p.extend(1.0);
    let x = (c.x / c.w * 0.5 + 0.5) * W as f32;
    let y = (0.5 - c.y / c.w * 0.5) * H as f32;
    (
        x.clamp(0.0, (W - 1) as f32) as u32,
        y.clamp(0.0, (H - 1) as f32) as u32,
    )
}

fn pixel(img: &[u8], (x, y): (u32, u32)) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// Whether a pixel is what the same camera sees with no frame at all (the
/// world pass draws its sky behind everything).
fn is_background(p: [u8; 4], sky: [u8; 4]) -> bool {
    (0..3).all(|i| p[i].abs_diff(sky[i]) <= 6)
}

/// `rgb / (r + g + b)`.
fn chroma(rgb: [f32; 3]) -> [f32; 3] {
    let s = (rgb[0] + rgb[1] + rgb[2]).max(1e-3);
    [rgb[0] / s, rgb[1] / s, rgb[2] / s]
}

fn chroma_px(p: [u8; 4]) -> [f32; 3] {
    chroma([f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}

/// Which of `refs` a pixel's chromaticity is nearest to.
fn nearest(p: [u8; 4], refs: &[[f32; 3]]) -> Option<usize> {
    let c = chroma_px(p);
    (0..refs.len()).min_by(|&a, &b| dist2(c, refs[a]).total_cmp(&dist2(c, refs[b])))
}

/// The mean colour of a jar texture's opaque texels in `(x0, y0, x1, y1)`.
fn jar_mean(jar: &Path, texture: &str, region: (usize, usize, usize, usize)) -> Option<[f32; 3]> {
    use std::io::Read;
    let f = std::fs::File::open(jar).ok()?;
    let mut z = zip::ZipArchive::new(f).ok()?;
    let mut bytes = Vec::new();
    z.by_name(&format!("assets/minecraft/textures/{texture}.png"))
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let (rgba, w, _) = crate::skin_fetch::decode_png_rgba(&bytes, (64, 64)).ok()?;
    let (mut sum, mut n) = ([0f32; 3], 0f32);
    for y in region.1..region.3 {
        for x in region.0..region.2 {
            let i = (y * w + x) * 4;
            if rgba[i + 3] > 0 {
                for k in 0..3 {
                    sum[k] += f32::from(rgba[i + k]);
                }
                n += 1.0;
            }
        }
    }
    (n > 0.0).then(|| sum.map(|s| s / n))
}

#[allow(clippy::too_many_arguments)]
fn check_pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    jar: &Path,
    baked: &assets::BakedAssets,
    etypes: &rewo_data::entity_types::EntityTypes,
    items: &rewo_data::items::Items,
    args: &ItemframeshotArgs,
) -> Result<(), String> {
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let back = chroma(jar_mean(jar, "block/item_frame", (3, 3, 13, 13)).ok_or("item_frame.png")?);
    let wood =
        chroma(jar_mean(jar, "block/birch_planks", (0, 0, 16, 16)).ok_or("birch_planks.png")?);
    let quad: Vec<[f32; 3]> = QUAD_PACKED
        .iter()
        .map(|&p| {
            let argb = rewo_data::map_colors::argb_from_packed(p);
            chroma([
                ((argb >> 16) & 0xFF) as f32,
                ((argb >> 8) & 0xFF) as f32,
                (argb & 0xFF) as f32,
            ])
        })
        .collect();

    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let mut off = Offscreen::new(gpu, W, H)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    // The production entity setup (glints, the map decoration sprites).
    crate::live_cmd::init_entities_maybe_cem(&mut wr, gpu, baked, &None)?;
    wr.set_held_items(crate::live_cmd::to_gpu_held_items(&baked.held_items));
    let lm = rewo_world::lightmap::LightmapState::default();

    let render = |gpu: &mut Gpu,
                  wr: &mut WorldRenderer,
                  off: &mut Offscreen,
                  inputs: &[FrameInputs],
                  maps: &MapStore,
                  facing: u8,
                  name: &str|
     -> Result<Vec<u8>, String> {
        let mut frames = ItemFrameDraws::default();
        for f in inputs {
            frame_draws(f, &lm, &mut frames);
        }
        let names: Vec<&str> = frames.models.iter().map(|m| m.model.as_str()).collect();
        wr.prepare_held_items(gpu, &names)?;
        prepare_frame_maps(wr, gpu, &frames, maps);
        let be: Vec<_> = frames.models.iter().map(|b| b.as_draw()).collect();
        let fm: Vec<_> = frames.maps.iter().map(|m| m.as_draw()).collect();
        let s = step_of(facing);
        let up = if s.y.abs() > 0.5 { Vec3::Z } else { Vec3::Y };
        wr.set_camera((s * DIST).to_array());
        let right = (-s).cross(up).normalize_or_zero().to_array();
        wr.set_entities_and_block_entities(&[], &be, &fm, &[], right, up.to_array(), 0.0);
        off.render(
            gpu,
            Some((&mut *wr, view_proj(facing).to_cols_array_2d())),
            &draw,
            CLEAR,
        )?;
        let img = off.read_rgba(gpu)?;
        if let Some(dir) = &args.out_dir {
            let _ = off.save_png(gpu, &dir.join(format!("frame-{name}.png")));
        }
        Ok(img)
    };
    let lit_frame =
        |facing: u8, rotation: u8, item: Option<&str>, map: Option<FramedMap>| FrameInputs {
            pos: [0.0; 3],
            facing,
            rotation,
            invisible: false,
            glow: false,
            item: item.map(str::to_string),
            foil: false,
            map,
            light: (15, 15),
            profile: None,
        };

    // p1-p3: an empty north frame. Its plate's front is at the entity's plane
    // (`0.46875 - 0.5 + 15.5/16` along the facing = 0), the plate spans
    // |a| < 0.3125 (texels 3..13), the wood |a| in 0.3125..0.375 (2..3 and
    // 13..14), and nothing past 0.375.
    let empty = render(
        gpu,
        &mut wr,
        &mut off,
        &[lit_frame(2, 0, None, None)],
        &MapStore::default(),
        2,
        "empty-north",
    )?;
    let vp = view_proj(2);
    let (a, b) = plane_axes(2);
    let at = |v: Vec3| pixel(&empty, project(&vp, v));
    let centre = at(Vec3::ZERO);
    c.record(
        "p1.an_empty_frame_shows_its_back_in_the_middle",
        nearest(centre, &[back, wood]) == Some(0),
        format!(
            "centre {centre:?}, chroma {:?} vs back {back:?} wood {wood:?}",
            chroma_px(centre)
        ),
    );
    let border: Vec<[u8; 4]> = [a * 0.344, a * -0.344, b * 0.344, b * -0.344]
        .into_iter()
        .map(at)
        .collect();
    c.record(
        "p2.its_border_is_birch_on_all_four_sides",
        border.iter().all(|&p| nearest(p, &[back, wood]) == Some(1)),
        format!("{border:?}"),
    );
    let nothing = render(
        gpu,
        &mut wr,
        &mut off,
        &[],
        &MapStore::default(),
        2,
        "nothing-north",
    )?;
    let sky_at = |v: Vec3| pixel(&nothing, project(&vp, v));
    let outside: Vec<([u8; 4], [u8; 4])> = [a * 0.42, b * -0.42]
        .into_iter()
        .map(|v| (at(v), sky_at(v)))
        .collect();
    c.record(
        "p3.nothing_past_the_border",
        outside.iter().all(|&(p, sky)| is_background(p, sky))
            && !is_background(centre, sky_at(Vec3::ZERO)),
        format!("{outside:?} (frame, empty scene) — and the centre is not the sky"),
    );

    // p4: every facing, seen from its front, is the same square — the plate
    // in the middle and birch at 0.344 along both in-plane axes. A frame
    // turned the wrong way is seen edge-on, and one pushed the wrong way
    // along its facing lands at another scale.
    let mut rows = Vec::new();
    let mut all = true;
    for facing in 0..6u8 {
        let img = render(
            gpu,
            &mut wr,
            &mut off,
            &[lit_frame(facing, 0, None, None)],
            &MapStore::default(),
            facing,
            &format!("facing{facing}"),
        )?;
        let sky = render(
            gpu,
            &mut wr,
            &mut off,
            &[],
            &MapStore::default(),
            facing,
            &format!("nothing{facing}"),
        )?;
        let vp = view_proj(facing);
        let (a, b) = plane_axes(facing);
        let at = |v: Vec3| pixel(&img, project(&vp, v));
        let ok = nearest(at(Vec3::ZERO), &[back, wood]) == Some(0)
            && [a * 0.344, b * 0.344, a * -0.344, b * -0.344]
                .into_iter()
                .all(|v| nearest(at(v), &[back, wood]) == Some(1))
            && is_background(at(a * 0.42), pixel(&sky, project(&vp, a * 0.42)));
        all &= ok;
        rows.push(format!("{facing}:{}", if ok { "ok" } else { "BAD" }));
    }
    c.record("p4.every_facing_faces_out", all, rows.join(" "));

    // p5/p6: a sword in a north frame. Its item plane is `step / 32` out; it
    // spans 0.5 blocks (`scale(0.5)` of a 1-block FIXED model), so a texel is
    // 1/32 of a block.
    let sword = render(
        gpu,
        &mut wr,
        &mut off,
        &[lit_frame(2, 0, Some("minecraft:diamond_sword"), None)],
        &MapStore::default(),
        2,
        "sword-r0",
    )?;
    let sword2 = render(
        gpu,
        &mut wr,
        &mut off,
        &[lit_frame(2, 2, Some("minecraft:diamond_sword"), None)],
        &MapStore::default(),
        2,
        "sword-r2",
    )?;
    let sword1 = render(
        gpu,
        &mut wr,
        &mut off,
        &[lit_frame(2, 1, Some("minecraft:diamond_sword"), None)],
        &MapStore::default(),
        2,
        "sword-r1",
    )?;
    let plane = step_of(2) / 32.0;
    let texel = |i: usize| ((i as f32 + 0.5) / 16.0 - 0.5) * 0.5;
    let blade = |p: [u8; 4]| {
        let k = chroma_px(p);
        k[1] > 0.38 && k[2] > 0.3
    };
    let mut on_blade = 0;
    let mut empty_blade = 0;
    let (mut agree, mut compared) = (0, 0);
    let mut differs45 = 0;
    for i in 0..16 {
        for j in 0..16 {
            // Local (x, y) of a north frame is world (x, y): `rotY(180 - 180)`.
            let (x, y) = (texel(i), texel(j));
            let p = plane + Vec3::new(x, y, 0.0);
            let s0 = pixel(&sword, project(&vp, p));
            if blade(s0) {
                on_blade += 1;
            }
            if blade(pixel(&empty, project(&vp, p))) {
                empty_blade += 1;
            }
            // `rotZ(rotation * 45)`: rotation 2 carries local (x, y) to (-y, x).
            let r = plane + Vec3::new(-y, x, 0.0);
            let s2 = pixel(&sword2, project(&vp, r));
            if blade(s0) || blade(s2) {
                compared += 1;
                if blade(s0) == blade(s2) {
                    agree += 1;
                }
            }
            if blade(s0) != blade(pixel(&sword1, project(&vp, p))) {
                differs45 += 1;
            }
        }
    }
    c.record(
        "p5.the_framed_sword_is_drawn",
        on_blade >= 20 && empty_blade == 0,
        format!("{on_blade} blade texels of 256 (want >= 20), {empty_blade} in the empty frame (want 0)"),
    );
    c.record(
        "p6.rotation_turns_it_in_eighths",
        compared >= 20 && agree * 100 >= compared * 90 && differs45 >= 10,
        format!(
            "rotation 2 = rotation 0 turned (x, y) -> (-y, x) at {agree} of {compared} blade \
             texels (want >= 90%); rotation 1 differs at {differs45} (want >= 10)"
        ),
    );

    // p10 (M198): a foil sword carries the entity glint over the item and
    // nowhere else — the frame around it is the plain render's, pixel for
    // pixel.
    let mut foil_in = lit_frame(2, 0, Some("minecraft:diamond_sword"), None);
    foil_in.foil = true;
    let foil = render(
        gpu,
        &mut wr,
        &mut off,
        &[foil_in],
        &MapStore::default(),
        2,
        "sword-foil",
    )?;
    let (mut lit_blade, mut blade_seen) = (0, 0);
    for i in 0..16 {
        for j in 0..16 {
            let p = plane + Vec3::new(texel(i), texel(j), 0.0);
            let at_px = project(&vp, p);
            if blade(pixel(&sword, at_px)) {
                blade_seen += 1;
                if pixel(&foil, at_px) != pixel(&sword, at_px) {
                    lit_blade += 1;
                }
            }
        }
    }
    let frame_same = [a * 0.344, a * -0.344, b * 0.344, b * -0.344]
        .into_iter()
        .all(|v| pixel(&foil, project(&vp, v)) == pixel(&sword, project(&vp, v)));
    c.record(
        "p10.a_foil_item_glints_and_its_frame_does_not",
        blade_seen >= 20 && lit_blade * 2 >= blade_seen && frame_same,
        format!(
            "{lit_blade} of {blade_seen} blade texels changed under the glint (want half or more); the frame's border unchanged: {frame_same}"
        ),
    );

    // p7-p9: the quadrant map in a north frame, driven through the collector.
    // `MapRenderer.render` after `submit`'s chain: map (u, v) lands at local
    // ((64 - u) / 128, (64 - v) / 128) (the `rotZ(180)` of
    // `scale(1/128) translate(-64, -64)`), then a quarter turn per rotation.
    let frame_ty = etypes
        .id_of("minecraft:item_frame")
        .ok_or("no item_frame type")?;
    let maps = quadrant_map(7);
    let map_scene = |rotation: i32| {
        let mut world = world_with(&[(1, frame_ty, [0.0, 0.0, 0.0], 2)]);
        world
            .entities
            .set_frame_item(1, frame_item(items, "minecraft:filled_map", Some(7)));
        world.entities.set_frame_rotation(1, rotation);
        let mut inputs = collect_frame_inputs(&world, &maps, None, etypes, items, 1.0);
        for f in &mut inputs {
            f.light = (15, 15);
        }
        inputs
    };
    let map_at = |u: f32, v: f32, quarter: u8| -> Vec3 {
        let (mut x, mut y) = ((64.0 - u) / 128.0, (64.0 - v) / 128.0);
        for _ in 0..quarter {
            (x, y) = (-y, x);
        }
        // The map plane: `0.46875 - 0.4375` = 1/32 out along the facing, plus
        // `translate(0, 0, -1)` of a map unit (1/128) further out.
        step_of(2) * (1.0 / 32.0 + 1.0 / 128.0) + Vec3::new(x, y, 0.0)
    };
    let centres = [
        (16.0f32, 16.0f32),
        (112.0, 16.0),
        (16.0, 112.0),
        (112.0, 112.0),
    ];
    let m0 = render(gpu, &mut wr, &mut off, &map_scene(0), &maps, 2, "map-r0")?;
    let got0: Vec<Option<usize>> = centres
        .iter()
        .map(|&(u, v)| nearest(pixel(&m0, project(&vp, map_at(u, v, 0))), &quad))
        .collect();
    c.record(
        "p7.the_map_is_upright_in_a_north_frame",
        got0 == [Some(0), Some(1), Some(2), Some(3)],
        format!("quadrants read {got0:?} (want red, blue, green, yellow = 0, 1, 2, 3)"),
    );
    let m1 = render(gpu, &mut wr, &mut off, &map_scene(1), &maps, 2, "map-r1")?;
    let got1: Vec<Option<usize>> = centres
        .iter()
        .map(|&(u, v)| nearest(pixel(&m1, project(&vp, map_at(u, v, 1))), &quad))
        .collect();
    // Rotation 5 is `5 % 4 = 1` quarter for a map (an item would turn 225).
    let m5 = render(gpu, &mut wr, &mut off, &map_scene(5), &maps, 2, "map-r5")?;
    let same5 = m5 == m1;
    c.record(
        "p8.a_map_turns_in_quarters",
        got1 == [Some(0), Some(1), Some(2), Some(3)] && same5,
        format!(
            "rotation 1 read {got1:?} at the quarter-turned points; rotation 5 identical: {same5}"
        ),
    );
    // The frame marker (`frame`, showOnItemFrame) sits at decoration (-64,
    // -64): `translate(x/2 + 64, y/2 + 64) scale(4) translate(-0.125, 0.125)`
    // puts its sprite's centre at map (31.5, 32.5), its green column on top
    // of the red quadrant. The player arrow at (64, -64) is not drawn, so its
    // spot stays blue.
    let marker: Vec<[u8; 4]> = [(32.0, 32.0), (32.0, 33.0), (32.0, 31.0)]
        .iter()
        .map(|&(u, v)| pixel(&m0, project(&vp, map_at(u, v, 0))))
        .collect();
    let green_marker = marker.iter().any(|&p| {
        let k = chroma_px(p);
        k[1] > 0.6
    });
    let arrow_spot = pixel(&m0, project(&vp, map_at(96.0, 32.5, 0)));
    c.record(
        "p9.only_the_frames_decorations_draw",
        green_marker && nearest(arrow_spot, &quad) == Some(1),
        format!(
            "marker {marker:?} (want a green one), player-arrow spot {arrow_spot:?} (want blue)"
        ),
    );

    // p11 (M204): a NAMED banner decoration (white banner, raw id 10,
    // showOnItemFrame) at half-pixels (-64, 40) — map (32, 84), in the green
    // quadrant — gets `MapRenderer`'s label: "Home" is about 23 font px wide,
    // so `scale = clamp(25 / width, 0, 6/9)` = 2/3 and the label spans map u
    // 24.5..39.5 from v 88 down six map px, on a half-black box with white
    // glyphs. Sampled at v 89..93 (below the 8 px banner sprite); without the
    // name the same spot is plain green.
    let mut named = quadrant_map(7);
    named.apply(&MapItemData {
        map_id: 7,
        scale: 0,
        locked: false,
        decorations: Some(vec![MapDecoration {
            type_id: 10,
            x: -64,
            y: 40,
            rot: 0,
            name: Some(rewo_proto::nbt::Nbt::String("Home".into())),
        }]),
        patch: None,
    });
    let named_scene = {
        let mut world = world_with(&[(1, frame_ty, [0.0, 0.0, 0.0], 2)]);
        world
            .entities
            .set_frame_item(1, frame_item(items, "minecraft:filled_map", Some(7)));
        let mut inputs = collect_frame_inputs(&world, &named, None, etypes, items, 1.0);
        for f in &mut inputs {
            f.light = (15, 15);
        }
        inputs
    };
    let labels = named_scene
        .first()
        .and_then(|f| f.map.as_ref())
        .map(|m| m.labels.clone())
        .unwrap_or_default();
    let ml = render(gpu, &mut wr, &mut off, &named_scene, &named, 2, "map-label")?;
    let (mut changed, mut white, mut points) = (0, 0, 0);
    for v in [89.0f32, 90.5, 92.0] {
        for i in 0..12 {
            let u = 26.0 + i as f32;
            let at_px = project(&vp, map_at(u, v, 0));
            let (a, b) = (pixel(&ml, at_px), pixel(&m0, at_px));
            points += 1;
            if a != b {
                changed += 1;
            }
            if a[0] > 150 && a[1] > 150 && a[2] > 150 {
                white += 1;
            }
        }
    }
    let plain_white = (0..12)
        .filter(|&i| {
            let p = pixel(&m0, project(&vp, map_at(26.0 + i as f32, 90.5, 0)));
            p[0] > 150 && p[1] > 150 && p[2] > 150
        })
        .count();
    c.record(
        "p11.a_named_banner_on_a_framed_map_is_labelled",
        labels.len() == 1
            && labels[0].text == "Home"
            && changed * 2 >= points
            && white >= 3
            && plain_white == 0,
        format!(
            "labels {labels:?}; under the banner {changed}/{points} points changed, {white} white glyph points (the unnamed map: {plain_white})"
        ),
    );

    wr.destroy(gpu);
    off.destroy(gpu);
    Ok(())
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}
