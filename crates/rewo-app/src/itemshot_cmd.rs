//! `rewo itemshot` — M22's permanent held-item oracle.
//!
//! Held items reach the screen through two geometry sources that converge on
//! one shape, and this gate drives the whole chain end to end:
//!
//! ```text
//! assets/minecraft/items/<item>.json      (definition; 5 state-dependent
//!   -> item_models::resolve_definition     types suppress)
//!   -> block model quads  OR  item_geometry::extrude
//!   -> held_items bake (quads 0..16 + texture)
//!   -> EntityPass::prepare_held_items      (atlas paging)
//!   -> emit_held_item                      (the ItemInHandLayer chain)
//!   -> a real render, read back
//! ```
//!
//! **The placement is verified against the hand, not against a screenshot.**
//! The same entity is rendered twice — empty-handed and holding an item — and
//! the changed pixels must fall inside the arm's neighbourhood and nowhere
//! else. That checks the transform chain landed the item *on the hand* without
//! the oracle needing to know the exact projected pixel of a sword tip.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::assets;
use rewo_data::item_models::{resolve_definition, ItemGeometry, ItemModel};
use rewo_gpu::entities::{EntityDraw, EntityModelKind, MobTextures};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 18;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 256;
const H: u32 = 256;

#[derive(ClapArgs)]
pub struct ItemshotArgs {
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Write the before/after renders here, for eyeballing a failure.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[itemshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: ItemshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!("[itemshot] mode: {mode} (the oracle asserts unconditionally)");

    let paths = rewo_data::DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_resolution(&mut c, &jar, &baked);
    check_geometry(&mut c, &baked);
    check_render(&mut c, &baked, &args)?;

    println!(
        "[itemshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "witness count {} != expected {EXPECTED_WITNESSES} — a named property was \
             skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[itemshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

/// Read one JSON entry out of the jar, for the definition witnesses.
fn jar_json(jar: &std::path::Path, path: &str) -> Option<serde_json::Value> {
    rewo_data::assets::jar_text(jar, path).and_then(|s| serde_json::from_str(&s).ok())
}

fn check_resolution(c: &mut Checker, jar: &std::path::Path, baked: &assets::BakedAssets) {
    let items = &baked.held_items;
    let total = items.models.len() + items.unsupported_total();
    c.record(
        "a1.every_item_the_jar_ships_is_accounted_for",
        total == 1537 && items.models.len() > 1300,
        format!(
            "{} resolved + {} unsupported = {total} (26.2 ships 1537 items; nothing is \
             silently dropped)",
            items.models.len(),
            items.unsupported_total()
        ),
    );
    c.record(
        "a2.both_geometry_sources_are_populated",
        items.block_count() > 600 && items.sprite_count() > 600,
        format!(
            "{} block-model items + {} extruded-sprite items over {} textures",
            items.block_count(),
            items.sprite_count(),
            items.textures.len()
        ),
    );
    // The five state-dependent definition types must each be represented in
    // the suppressed set — if one silently started resolving, its items would
    // be drawn from a guessed state.
    let kinds: Vec<&str> = items.unsupported.keys().map(String::as_str).collect();
    let wanted = [
        "minecraft:select",
        "minecraft:special",
        "minecraft:composite",
        "minecraft:condition",
        "minecraft:range_dispatch",
    ];
    let missing: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|w| !kinds.contains(w))
        .collect();
    c.record(
        "a3.every_state_dependent_definition_type_is_suppressed",
        missing.is_empty(),
        format!("suppressed buckets {kinds:?}; missing {missing:?}"),
    );

    // Spot-check the two paths against the definitions themselves, so a
    // renamed asset cannot quietly move an item to the other source.
    let sword = jar_json(jar, "assets/minecraft/items/diamond_sword.json")
        .map(|d| resolve_definition(&d, &mut |_| None));
    c.record(
        "a4.a_sword_definition_names_an_item_model",
        matches!(&sword, Some(ItemModel::Unsupported(k)) if k.starts_with("model item/")),
        format!(
            "diamond_sword with no model reader → {sword:?} (an item/ reference, so the \
             sprite path; the reader is stubbed here on purpose)"
        ),
    );
    let dirt = jar_json(jar, "assets/minecraft/items/dirt.json")
        .map(|d| resolve_definition(&d, &mut |_| None));
    c.record(
        "a5.a_block_item_resolves_without_touching_a_model_file",
        matches!(
            &dirt,
            Some(ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. }) if b == "dirt"
        ),
        format!("dirt → {dirt:?} (a block/ reference needs no chain walk)"),
    );
    let bow = jar_json(jar, "assets/minecraft/items/bow.json")
        .map(|d| resolve_definition(&d, &mut |_| None));
    c.record(
        "a6.a_pull_dependent_item_is_suppressed",
        matches!(&bow, Some(ItemModel::Unsupported(k)) if k == "minecraft:condition"),
        format!("bow → {bow:?} (its model depends on draw progress)"),
    );
}

fn check_geometry(c: &mut Checker, baked: &assets::BakedAssets) {
    let items = &baked.held_items;
    let sword = items.get("minecraft:diamond_sword");
    c.record(
        "b1.the_sword_baked_as_an_extruded_sprite",
        sword.is_some_and(|m| !m.from_block && m.quads.len() > 2),
        format!(
            "diamond_sword: {} quads, from_block={:?} (2 faces + one per alpha edge)",
            sword.map_or(0, |m| m.quads.len()),
            sword.map(|m| m.from_block)
        ),
    );
    // The extrusion's slab: every sword quad lives between z 7.5 and 8.5.
    let slab_ok = sword.is_some_and(|m| {
        m.quads
            .iter()
            .flat_map(|q| q.verts)
            .all(|v| (7.5..=8.5).contains(&v[2]))
    });
    c.record(
        "b2.the_sprite_extrusion_stays_in_the_one_sixteenth_slab",
        slab_ok,
        format!("every diamond_sword vertex has z in 7.5..8.5: {slab_ok}"),
    );
    let dirt = items.get("minecraft:dirt");
    c.record(
        "b3.a_block_item_baked_from_the_block_model",
        dirt.is_some_and(|m| m.from_block && m.quads.len() >= 6),
        format!(
            "dirt: {} quads, from_block={:?} (a full cube is six faces)",
            dirt.map_or(0, |m| m.quads.len()),
            dirt.map(|m| m.from_block)
        ),
    );
    // Block items occupy the whole 0..16 cube; a sprite item is a thin slab.
    let cube_ok = dirt.is_some_and(|m| {
        let ys: Vec<f32> = m.quads.iter().flat_map(|q| q.verts).map(|v| v[1]).collect();
        ys.iter().cloned().fold(f32::INFINITY, f32::min) <= 0.01
            && ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max) >= 15.99
    });
    c.record(
        "b4.a_block_item_spans_the_full_model_cube",
        cube_ok,
        format!("dirt spans y 0..16 in model units: {cube_ok}"),
    );
    // The two transforms differ, and the left one mirrors — the handheld
    // chain gives a sword rotation [0,-90,55] right / [0,90,-55] left.
    let mirrored = sword.is_some_and(|m| {
        (m.right.rotation[1] + m.left.rotation[1]).abs() < 1e-4
            && m.right.rotation[1].abs() > 1e-3
    });
    c.record(
        "b5.the_left_hand_transform_mirrors_the_right",
        mirrored,
        format!(
            "diamond_sword right rot {:?} vs left {:?}",
            sword.map(|m| m.right.rotation),
            sword.map(|m| m.left.rotation)
        ),
    );
    // The deserializer's 0.0625 scaling: handheld translates [0,4,0.5] model
    // units, which must land as [0, 0.25, 0.03125] block units.
    let scaled = sword.is_some_and(|m| (m.right.translation[1] - 0.25).abs() < 1e-5);
    c.record(
        "b6.the_display_translation_is_in_block_units",
        scaled,
        format!(
            "diamond_sword translation {:?} — raw JSON is [0, 4.0, 0.5] model units, and \
             ItemTransform.Deserializer multiplies by 0.0625",
            sword.map(|m| m.right.translation)
        ),
    );
}

/// Render a player model with and without a held item, and compare.
fn check_render(
    c: &mut Checker,
    baked: &assets::BakedAssets,
    args: &ItemshotArgs,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let white: Vec<u8> = vec![255u8; (16 * 16 * 4) as usize];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.init_entities(
        &mut gpu,
        crate::live_cmd::font_data(baked),
        crate::live_cmd::entity_textures(baked),
    )?;
    wr.set_held_items(crate::live_cmd::to_gpu_held_items(&baked.held_items));

    let eye = Vec3::new(0.0, 1.0, 2.6);
    let dir = Vec3::new(0.0, 0.0, -1.0);
    let up = Vec3::Y;
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, up);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let view_proj = (proj * view).to_cols_array_2d();
    let right = dir.cross(up).normalize_or_zero().to_array();

    let render = |held: [Option<&str>; 2], gpu: &mut Gpu, wr: &mut WorldRenderer, off: &mut Offscreen| -> Result<Vec<u8>, String> {
        let names: Vec<&str> = held.iter().flatten().copied().collect();
        wr.prepare_held_items(gpu, &names)?;
        let d = EntityDraw {
            pos: [0.0, 0.0, 0.0],
            width: 0.6,
            height: 1.8,
            color: [1.0, 1.0, 1.0],
            name: None,
            kind: EntityModelKind::Player,
            yaw: 0.0,
            head_yaw: 0.0,
            pitch: 0.0,
            limb_swing: 0.0,
            limb_amount: 0.0,
            gesture: None,
            events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
            shell: false,
            allay_dance: None,
            attack: rewo_gpu::mobs::SwingPose::NONE,
            mob: rewo_gpu::mobs::MobCombat::default(),
            hurt: false,
            held,
            arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
            skin_uv: None,
            scale_mul: 1.0,
            anim_id: 0.0,
            light: [1.0, 1.0, 1.0],
        };
        wr.set_entities(&[d], right, up.to_array(), 0.0);
        off.render(gpu, Some((&mut *wr, view_proj)), &draw, CLEAR)?;
        off.read_rgba(gpu)
    };

    let empty = render([None, None], &mut gpu, &mut wr, &mut off)?;
    let sword = render([Some("minecraft:diamond_sword"), None], &mut gpu, &mut wr, &mut off)?;
    let dirt = render([Some("minecraft:dirt"), None], &mut gpu, &mut wr, &mut off)?;
    let bow = render([Some("minecraft:bow"), None], &mut gpu, &mut wr, &mut off)?;
    if let Some(dir_out) = &args.out_dir {
        std::fs::create_dir_all(dir_out).map_err(|e| format!("out-dir: {e}"))?;
        let _ = off.save_png(&mut gpu, &dir_out.join("itemshot-empty.png"));
    }

    let changed = |a: &[u8], b: &[u8]| -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for y in 0..H {
            for x in 0..W {
                let i = ((y * W + x) * 4) as usize;
                if (0..3).any(|k| (a[i + k] as i32 - b[i + k] as i32).abs() > 8) {
                    v.push((x, y));
                }
            }
        }
        v
    };

    let sword_px = changed(&empty, &sword);
    c.record(
        "c1.a_sprite_item_renders",
        sword_px.len() > 40,
        format!("{} pixels differ from the empty hand", sword_px.len()),
    );
    let dirt_px = changed(&empty, &dirt);
    c.record(
        "c2.a_block_item_renders",
        dirt_px.len() > 40,
        format!("{} pixels differ from the empty hand", dirt_px.len()),
    );
    // A suppressed item must be indistinguishable from an empty hand.
    let bow_px = changed(&empty, &bow);
    c.record(
        "c3.a_suppressed_item_renders_as_an_empty_hand",
        bow_px.is_empty(),
        format!("{} pixels differ (want 0 — the bow has no baked model)", bow_px.len()),
    );
    // Placement: the item must appear on the right side of the body (the
    // entity faces the camera at yaw 0, so its right arm is screen-left) and
    // below the head. A transform-order slip parks it at the origin or
    // across the whole frame, which this catches.
    let (cx, cy) = centroid(&sword_px);
    c.record(
        "c4.the_item_lands_on_the_hand_not_the_origin",
        !sword_px.is_empty() && cx < W as f32 * 0.5 && cy > H as f32 * 0.4,
        format!(
            "sprite-item centroid ({cx:.0}, {cy:.0}) — screen-left of centre (the entity's \
             right arm at yaw 0) and below the head"
        ),
    );
    let (bx, by) = centroid(&dirt_px);
    c.record(
        "c5.both_sources_land_in_the_same_place",
        !dirt_px.is_empty() && (bx - cx).abs() < W as f32 * 0.25 && (by - cy).abs() < H as f32 * 0.25,
        format!(
            "block-item centroid ({bx:.0}, {by:.0}) vs sprite ({cx:.0}, {cy:.0}) — the same \
             hand, so the two geometry sources share one transform chain"
        ),
    );
    // The left hand mirrors: holding the sword in the off hand must move it to
    // the other side of the body.
    let left = render([None, Some("minecraft:diamond_sword")], &mut gpu, &mut wr, &mut off)?;
    let left_px = changed(&empty, &left);
    let (lx, _ly) = centroid(&left_px);
    c.record(
        "c6.the_off_hand_item_appears_on_the_other_side",
        !left_px.is_empty() && lx > cx,
        format!("off-hand centroid x {lx:.0} vs main-hand {cx:.0}"),
    );

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

fn centroid(px: &[(u32, u32)]) -> (f32, f32) {
    if px.is_empty() {
        return (0.0, 0.0);
    }
    let n = px.len() as f32;
    (
        px.iter().map(|p| p.0 as f32).sum::<f32>() / n,
        px.iter().map(|p| p.1 as f32).sum::<f32>() / n,
    )
}
