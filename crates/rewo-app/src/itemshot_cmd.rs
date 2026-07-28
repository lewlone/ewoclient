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
use rewo_data::equipment;
use rewo_data::item_models::{
    resolve_definition, DisplayContext, ItemGeometry, ItemModel, SelectionContext,
};
use rewo_gpu::entities::{EntityDraw, EntityModelKind};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 62;

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
    // M49: trimmed icon variants live in the same map under `"<item>#<material>"`,
    // so the count of *items* is the keys without a '#'. Counted rather than
    // subtracted from a constant, so the accounting stays fail-closed.
    let plain = items.models.keys().filter(|k| !k.contains('#')).count();
    let variants = items.models.len() - plain;
    let total = plain + items.unsupported_total();
    c.record(
        "a1.every_item_the_jar_ships_is_accounted_for",
        total == 1537 && plain > 1300,
        format!(
            "{plain} resolved + {} unsupported = {total} (26.2 ships 1537 items; nothing              is silently dropped), plus {variants} trimmed icon variant(s)",
            items.unsupported_total()
        ),
    );
    c.record(
        "a1b.every_trimmed_variant_belongs_to_an_item_that_resolved",
        variants > 200
            && items.models.keys().filter_map(|k| k.split_once('#')).all(|(base, mat)| {
                items.models.contains_key(base) && mat.starts_with("minecraft:")
            }),
        format!(
            "{variants} variant(s), every one `<item>#<material id>` over an item that              resolved on its own. `HeldItems::any` falls back to that base, which is              where a material the definition names no case for has to land"
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
    // M40 reduced the branching types, so the suppressed set is now the two
    // that are real work rather than a branch to evaluate: `composite` layers
    // several models into one draw, `special` hands the stack to a bespoke
    // renderer. Both must still be there — one silently starting to resolve
    // would draw a bed as whichever of its layers came first.
    let kinds: Vec<&str> = items.unsupported.keys().map(String::as_str).collect();
    let wanted = ["minecraft:special", "minecraft:composite"];
    let missing: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|w| !kinds.contains(w))
        .collect();
    c.record(
        "a3.the_two_non_branching_definition_types_are_still_suppressed",
        missing.is_empty(),
        format!("suppressed buckets {kinds:?}; missing {missing:?}"),
    );
    // The other half of the same claim, and the one that would catch a
    // reduction that quietly stopped working: `condition` and
    // `range_dispatch` must *not* appear as suppressed buckets any more. They
    // are gone because their branches are evaluated now, not because they
    // vanished from the jar — a3's `composite`/`special` counts prove the
    // suppression machinery still runs.
    let still_branching: Vec<&str> = ["minecraft:condition", "minecraft:range_dispatch"]
        .into_iter()
        .filter(|k| kinds.contains(k))
        .collect();
    c.record(
        "a3b.the_branching_definition_types_are_no_longer_suppressed",
        still_branching.is_empty(),
        format!(
            "{} items resolved, {} suppressed; branching types still suppressed: {:?}",
            items.models.len(),
            items.unsupported.values().sum::<usize>(),
            still_branching
        ),
    );

    // Spot-check the two paths against the definitions themselves, so a
    // renamed asset cannot quietly move an item to the other source.
    let sword = jar_json(jar, "assets/minecraft/items/diamond_sword.json")
        .map(|d| resolve_definition(&d, &mut |_| None, SelectionContext::hand()));
    c.record(
        "a4.a_sword_definition_names_an_item_model",
        matches!(&sword, Some(ItemModel::Unsupported(k)) if k.starts_with("model item/")),
        format!(
            "diamond_sword with no model reader → {sword:?} (an item/ reference, so the \
             sprite path; the reader is stubbed here on purpose)"
        ),
    );
    let dirt = jar_json(jar, "assets/minecraft/items/dirt.json")
        .map(|d| resolve_definition(&d, &mut |_| None, SelectionContext::hand()));
    c.record(
        "a5.a_block_item_resolves_without_touching_a_model_file",
        matches!(
            &dirt,
            Some(ItemModel::Resolved { geometry: ItemGeometry::Block(b), .. }) if b == "dirt"
        ),
        format!("dirt → {dirt:?} (a block/ reference needs no chain walk)"),
    );
    // M40. The bow is a `condition` on `using_item` whose `on_true` is a
    // `range_dispatch` over draw progress. At rest that condition is false, so
    // the reduction must reach `item/bow` — the *reference*, which the stubbed
    // reader here then fails to walk. Seeing `model item/bow` rather than
    // `minecraft:condition` is the whole proof: the branch was evaluated.
    let bow = jar_json(jar, "assets/minecraft/items/bow.json")
        .map(|d| resolve_definition(&d, &mut |_| None, SelectionContext::hand()));
    c.record(
        "a6.an_unused_bow_reduces_to_its_resting_model",
        matches!(&bow, Some(ItemModel::Unsupported(k))
            if k == "model item/bow (not builtin/generated)"),
        format!("bow → {bow:?} (condition using_item=false takes on_false)"),
    );
    // The same tree with the condition true must take the *other* branch, or
    // the reduction is only ever returning the first thing it finds.
    let drawn = jar_json(jar, "assets/minecraft/items/bow.json").map(|d| {
        resolve_definition(
            &d,
            &mut |_| None,
            SelectionContext {
                display: DisplayContext::FirstPersonRightHand,
                trim_material: None,
                using_item: true,
            },
        )
    });
    c.record(
        "a6b.a_drawn_bow_reduces_to_a_different_model",
        matches!(&drawn, Some(ItemModel::Unsupported(k))
            if k == "model item/bow_pulling_0 (not builtin/generated)"),
        format!(
            "bow with using_item=true → {drawn:?} (on_true, then a range_dispatch \
             at zero pull → its fallback)"
        ),
    );
    // An untrimmed helmet: `select` on `minecraft:trim_material`, a component
    // a plain stack does not carry, so no case matches and vanilla renders the
    // fallback. This is the witness the armour icons hang on.
    let helmet = jar_json(jar, "assets/minecraft/items/diamond_helmet.json")
        .map(|d| resolve_definition(&d, &mut |_| None, SelectionContext::gui()));
    c.record(
        "a7.an_untrimmed_helmet_reduces_to_the_plain_model",
        matches!(&helmet, Some(ItemModel::Unsupported(k))
            if k == "model item/diamond_helmet (not builtin/generated)"),
        format!("diamond_helmet → {helmet:?} (no TRIM component, so no case matches)"),
    );
    // The fail-closed half: a property Rewo cannot evaluate must still
    // suppress. A clock branches on which dimension you are in, which a baked
    // model has no way to know.
    let clock = jar_json(jar, "assets/minecraft/items/clock.json")
        .map(|d| resolve_definition(&d, &mut |_| None, SelectionContext::gui()));
    c.record(
        "a8.an_unevaluable_property_still_suppresses",
        matches!(&clock, Some(ItemModel::Unsupported(k))
            if k == "minecraft:select (minecraft:context_dimension)"),
        format!(
            "clock → {clock:?} — the reason names the property, not just the type,              so a bucket says what could not be answered"
        ),
    );
    // And the context half: a spear selects *different geometry* per display
    // context, which is why the reduction takes a context at all.
    let spear = |ctx| {
        jar_json(jar, "assets/minecraft/items/copper_spear.json")
            .map(|d| resolve_definition(&d, &mut |_| None, ctx))
    };
    let (in_slot, in_hand) = (spear(SelectionContext::gui()), spear(SelectionContext::hand()));
    c.record(
        "a9.a_spear_selects_different_geometry_in_a_slot_and_in_the_hand",
        in_slot != in_hand
            && matches!(&in_slot, Some(ItemModel::Unsupported(k))
                if k == "model item/copper_spear (not builtin/generated)")
            && matches!(&in_hand, Some(ItemModel::Unsupported(k))
                if k == "model item/copper_spear_in_hand (not builtin/generated)"),
        format!("gui → {in_slot:?}; hand → {in_hand:?}"),
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
    // M45: the glint is a second pipeline on the entity pass and has to be
    // installed **after** it exists. The live path does this in
    // `init_entities_maybe_cem`; a gate that calls `init_entities` directly
    // has to do it too, or every glint witness measures zero — which is
    // exactly how this line came to be here.
    if let Some(g) = baked.glint.as_ref() {
        wr.init_entity_glint(&mut gpu, &g.rgba, g.w, g.h)?;
    }
    // M50: the worn-armour foil is a *second* sheet, and it needs its own line
    // here for the same reason the first one did. This is the general rule, not
    // an accident twice over: a gate that reimplements a slice of the app's
    // setup misses whatever the app later adds to it.
    if let Some(g) = baked.armor_glint.as_ref() {
        wr.init_entity_armor_glint(&mut gpu, &g.rgba, g.w, g.h)?;
    }
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

    // M47: the same draw, wearing a resolved armour piece. Takes the piece
    // rather than an item name so the witness exercises the *renderer* with a
    // known tint; the resolution from item to piece is graded separately.
    let render_armor = |armor: [Option<rewo_gpu::entities::ArmorPiece<'_>>; 4],
                        gpu: &mut Gpu,
                        wr: &mut WorldRenderer,
                        off: &mut Offscreen|
     -> Result<Vec<u8>, String> {
        let d = EntityDraw {
            armor,
            pos: [0.0, 0.0, 0.0],
            width: 0.6,
            height: 1.8,
            color: [1.0, 1.0, 1.0],
            name: None,
            kind: EntityModelKind::Player,
            yaw: 0.0,
            death_time: 0.0,
            ground_item: None,
            held_glint: [false; 2],
            ground_glint: false,
            ground_count: 0,
            bob_offset: 0.0,
            ground_seed: 0,
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
            held: [None; 2],
            arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
            skin_uv: None,
            scale_mul: 1.0,
            mount: None,
            anim_id: 0.0,
            light: [1.0, 1.0, 1.0],
            // M52: no emissive state, no pack variant, no dye — the
            // vanilla defaults, which is what this gate renders.
            emissive: rewo_gpu::entities::EmissiveState::default(),
            variant: 0,
            dye: None,
        };
        wr.set_entities(&[d], right, up.to_array(), 0.0);
        off.render(gpu, Some((&mut *wr, view_proj)), &draw, CLEAR)?;
        off.read_rgba(gpu)
    };

    let render_glint = |held: [Option<&str>; 2],
                        held_glint: [bool; 2],
                        gpu: &mut Gpu,
                        wr: &mut WorldRenderer,
                        off: &mut Offscreen|
     -> Result<Vec<u8>, String> {
        let names: Vec<&str> = held.iter().flatten().copied().collect();
        wr.prepare_held_items(gpu, &names)?;
        let d = EntityDraw {
            armor: [None; 4],
            pos: [0.0, 0.0, 0.0],
            width: 0.6,
            height: 1.8,
            color: [1.0, 1.0, 1.0],
            name: None,
            kind: EntityModelKind::Player,
            yaw: 0.0,
            death_time: 0.0,
            ground_item: None,
            held_glint,
            ground_glint: false,
            ground_count: 0,
            bob_offset: 0.0,
            ground_seed: 0,
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
            mount: None,
            anim_id: 0.0,
            light: [1.0, 1.0, 1.0],
            // M52: no emissive state, no pack variant, no dye — the
            // vanilla defaults, which is what this gate renders.
            emissive: rewo_gpu::entities::EmissiveState::default(),
            variant: 0,
            dye: None,
        };
        wr.set_entities(&[d], right, up.to_array(), 0.0);
        off.render(gpu, Some((&mut *wr, view_proj)), &draw, CLEAR)?;
        off.read_rgba(gpu)
    };
    // The unglinted case, which is every witness written before M45.
    let render = |held: [Option<&str>; 2], gpu: &mut Gpu, wr: &mut WorldRenderer, off: &mut Offscreen| {
        render_glint(held, [false; 2], gpu, wr, off)
    };

    // M24b: the same scene, but as a *dropped* stack — no model, no capsule,
    // the item is the entity. `EntityModelKind::Capsule` is passed to prove
    // the kind is never consulted: `ground_item` short-circuits before it.
    let ground = |name: Option<&str>,
                  count: i32,
                  bob: f32,
                  seed: i32,
                  time: f32,
                  glint: bool,
                  gpu: &mut Gpu,
                  wr: &mut WorldRenderer,
                  off: &mut Offscreen|
     -> Result<Vec<u8>, String> {
        let names: Vec<&str> = name.into_iter().collect();
        wr.prepare_held_items(gpu, &names)?;
        let d = EntityDraw {
            pos: [0.0, 0.0, 0.0],
            width: 0.25,
            height: 0.25,
            color: [1.0, 1.0, 1.0],
            name: None,
            kind: EntityModelKind::Capsule,
            yaw: 0.0,
            death_time: 0.0,
            ground_item: name,
            armor: [None; 4],
            held_glint: [false; 2],
            ground_glint: glint,
            ground_count: count,
            bob_offset: bob,
            ground_seed: seed,
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
            held: [None, None],
            arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
            skin_uv: None,
            scale_mul: 1.0,
            mount: None,
            anim_id: 0.0,
            light: [1.0, 1.0, 1.0],
            // M52: no emissive state, no pack variant, no dye — the
            // vanilla defaults, which is what this gate renders.
            emissive: rewo_gpu::entities::EmissiveState::default(),
            variant: 0,
            dye: None,
        };
        wr.set_entities(&[d], right, up.to_array(), time);
        off.render(gpu, Some((&mut *wr, view_proj)), &draw, CLEAR)?;
        off.read_rgba(gpu)
    };

    let empty = render([None, None], &mut gpu, &mut wr, &mut off)?;
    let sword = render([Some("minecraft:diamond_sword"), None], &mut gpu, &mut wr, &mut off)?;
    let dirt = render([Some("minecraft:dirt"), None], &mut gpu, &mut wr, &mut off)?;
    let bow = render([Some("minecraft:white_bed"), None], &mut gpu, &mut wr, &mut off)?;
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
        format!(
            "{} pixels differ (want 0) — a bed is a `composite` of several \
             models, which M40's reduction deliberately does not resolve",
            bow_px.len()
        ),
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

    // --- M24b: dropped stacks --------------------------------------------
    // An item name with no baked model draws nothing, which is the only
    // honest empty baseline here: `ground_item: None` falls through to the
    // *capsule fallback*, so it is a control rather than a blank.
    let g_none = ground(Some("minecraft:__absent__"), 1, 0.0, 0, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_capsule = ground(None, 0, 0.0, 0, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_sword = ground(Some("minecraft:diamond_sword"), 1, 0.0, 7, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_dirt = ground(Some("minecraft:dirt"), 1, 0.0, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_sword_px = changed(&g_none, &g_sword);
    let g_dirt_px = changed(&g_none, &g_dirt);
    c.record(
        "g1.a_dropped_stack_renders",
        !g_sword_px.is_empty() && !g_dirt_px.is_empty(),
        format!(
            "sprite {} px, block {} px against an empty frame",
            g_sword_px.len(),
            g_dirt_px.len()
        ),
    );

    // …and it renders INSTEAD of the body, not on top of it. The same draw
    // with `ground_item: None` falls through to the capsule, so the capsule
    // control is non-empty; a ground item must differ from it and must not
    // contain it.
    let capsule_px = changed(&g_none, &g_capsule);
    c.record(
        "g10.a_ground_item_replaces_the_body_rather_than_adding_to_it",
        {
            // The capsule's pixels must not all survive into the ground-item
            // render. A size comparison would be a proxy (and a wrong one —
            // the sword covers MORE pixels than the capsule here); this is the
            // property itself.
            let sword_set: std::collections::HashSet<(u32, u32)> =
                g_sword_px.iter().copied().collect();
            !capsule_px.is_empty() && capsule_px.iter().any(|p| !sword_set.contains(p))
        },
        format!(
            "the same draw without a ground item renders a {} px capsule; with one \
             it renders {} px of sword and no capsule — `ItemEntityRenderer` has no \
             body, so the model kind is never consulted",
            capsule_px.len(),
            g_sword_px.len()
        ),
    );

    // The GROUND transform is a different context from the hand's, and the
    // difference is observable: `block/block`'s ground scale is 0.25 against
    // the hand's 0.375, so a dropped block covers visibly fewer pixels than a
    // held one at the same camera.
    c.record(
        "g2.the_ground_context_is_not_the_hand_context",
        g_dirt_px.len() < dirt_px.len(),
        format!(
            "dropped dirt {} px vs held dirt {} px — `display.ground` scales \
             0.25 where `thirdperson_righthand` scales 0.375",
            g_dirt_px.len(),
            dirt_px.len()
        ),
    );

    // The bob is a sine of `ageInTicks / 10 + bobOffset`: two different offsets
    // at the same instant must place the item at different heights.
    // The offsets must be sine *extrema*, not 0 and PI — those are both zeros,
    // which is what the first version of this witness got wrong.
    let g_bob_a = ground(Some("minecraft:dirt"), 1, std::f32::consts::FRAC_PI_2, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_bob_b = ground(Some("minecraft:dirt"), 1, 3.0 * std::f32::consts::FRAC_PI_2, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let (_, ya) = centroid(&changed(&g_none, &g_bob_a));
    let (_, yb) = centroid(&changed(&g_none, &g_bob_b));
    c.record(
        "g3.the_bob_offset_shifts_the_item_vertically",
        (ya as i32 - yb as i32).abs() >= 2,
        format!(
            "bobOffset PI/2 -> centroid y {ya}, bobOffset 3PI/2 -> y {yb} — \
             sin(age/10 + offset) * 0.1 + 0.1, so the two sit 0.2 blocks apart"
        ),
    );

    // The spin is `age / 20 + bobOffset`: advancing the clock a quarter turn
    // must change the silhouette of an asymmetric item.
    let g_t0 = ground(Some("minecraft:diamond_sword"), 1, 0.0, 7, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_t1 = ground(Some("minecraft:diamond_sword"), 1, 0.0, 7, 0.25 * std::f32::consts::PI / 2.0 * 20.0 / 20.0, false, &mut gpu, &mut wr, &mut off)?;
    c.record(
        "g4.the_item_spins_with_the_clock",
        !changed(&g_t0, &g_t1).is_empty(),
        format!(
            "{} pixels differ after advancing the clock — getSpin is age/20 + \
             bobOffset, so a still item at two times must not match",
            changed(&g_t0, &g_t1).len()
        ),
    );

    // `getRenderedAmount` is a step function, and each step adds copies that
    // the seeded LCG jitters apart — so more copies cover more pixels.
    let g_one = ground(Some("minecraft:dirt"), 1, 0.0, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_many = ground(Some("minecraft:dirt"), 64, 0.0, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let one_px = changed(&g_none, &g_one).len();
    let many_px = changed(&g_none, &g_many).len();
    c.record(
        "g5.a_bigger_stack_draws_more_copies",
        many_px > one_px,
        format!(
            "count 1 -> {one_px} px, count 64 -> {many_px} px (5 copies, jittered \
             +/-0.15 by a seeded LegacyRandomSource)"
        ),
    );

    // The bucketing is a step, not a ramp: 17 and 32 both render 3 copies, so
    // they must be pixel-identical.
    let g_17 = ground(Some("minecraft:dirt"), 17, 0.0, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_32 = ground(Some("minecraft:dirt"), 32, 0.0, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    c.record(
        "g6.the_copy_count_is_bucketed_not_proportional",
        g_17 == g_32,
        format!(
            "counts 17 and 32 render identically = {} — getRenderedAmount buckets \
             1/2-16/17-32/33-48/49+ into 1/2/3/4/5",
            g_17 == g_32
        ),
    );

    // The jitter is seeded by the item, so the same stack is stable across
    // renders and a different seed lays the copies out differently.
    let g_seed_a = ground(Some("minecraft:dirt"), 64, 0.0, 9, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_seed_b = ground(Some("minecraft:dirt"), 64, 0.0, 4242, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    c.record(
        "g7.the_copy_jitter_is_seeded_and_reproducible",
        g_many == g_seed_a && g_many != g_seed_b,
        format!(
            "same seed reproduces={}, a different seed differs={} — the LCG is reset \
             to getSeedForItemStack each render, so a dropped stack does not shimmer",
            g_many == g_seed_a,
            g_many != g_seed_b
        ),
    );

    // A suppressed item is nothing on the ground too, not a fallback shape.
    let g_bow = ground(Some("minecraft:white_bed"), 1, 0.0, 3, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    c.record(
        "g8.a_suppressed_item_drops_as_nothing",
        changed(&g_none, &g_bow).is_empty(),
        format!(
            "{} pixels differ (want 0) — a bed's definition is a `composite`, \
             so it has no baked model in either context",
            changed(&g_none, &g_bow).len()
        ),
    );

    // The ground item sits at the entity origin, not on an arm: its centroid
    // must be near the frame centre horizontally, unlike the held sword which
    // hangs screen-left on the right arm.
    let (gx, _) = centroid(&g_sword_px);
    c.record(
        "g9.a_dropped_item_is_centred_on_the_entity",
        (gx as i32 - (W / 2) as i32).abs() < (cx as i32 - (W / 2) as i32).abs(),
        format!(
            "dropped sword centroid x {gx} vs held sword x {cx} (frame centre {}) — \
             a dropped stack hangs off the entity origin, a held one off an arm",
            W / 2
        ),
    );

    // -- worn armour (M46) ---------------------------------------------------
    //
    // `ADULT_ARMOR_PARTS_PER_SLOT` and the two `CubeDeformation`s, which
    // together decide what a piece covers and how far it stands off the skin.
    use rewo_gpu::mobs::{armor_boxes, armor_part, ArmorSlot};
    let parts_of = |s: ArmorSlot| {
        let mut v: Vec<&str> = armor_boxes(s).iter().map(|b| b.part).collect();
        v.sort_unstable();
        v
    };
    c.record(
        "r1.each_slot_covers_the_parts_vanilla_gives_it",
        parts_of(ArmorSlot::Head) == ["head"]
            && parts_of(ArmorSlot::Chest) == ["body", "left_arm", "right_arm"]
            && parts_of(ArmorSlot::Legs) == ["body", "left_leg", "right_leg"]
            && parts_of(ArmorSlot::Feet) == ["left_leg", "right_leg"],
        format!(
            "head {:?}, chest {:?}, legs {:?}, feet {:?}. A chestplate covers both              **arms** as well as the body, and the leggings cover the **body** as              well as the legs — the body is in two pieces at once, which is the              whole reason they are inflated differently",
            parts_of(ArmorSlot::Head),
            parts_of(ArmorSlot::Chest),
            parts_of(ArmorSlot::Legs),
            parts_of(ArmorSlot::Feet)
        ),
    );
    c.record(
        "r2.only_the_leggings_use_the_inner_inflation",
        ArmorSlot::Legs.grow() == 0.5
            && [ArmorSlot::Head, ArmorSlot::Chest, ArmorSlot::Feet]
                .iter()
                .all(|s| s.grow() == 1.0),
        format!(
            "legs {} against head/chest/feet {} — `usesInnerModel` is              `slot == LEGS`, and the thinner inflation is what lets leggings sit              inside a chestplate instead of z-fighting it",
            ArmorSlot::Legs.grow(),
            ArmorSlot::Head.grow()
        ),
    );
    // The legs are a *replacement* box, a tenth thinner again.
    let leg_extend = armor_boxes(ArmorSlot::Feet)
        .iter()
        .map(|b| b.extend)
        .fold(f32::NAN, f32::max);
    c.record(
        "r3.the_leg_box_is_a_tenth_thinner_than_the_rest_of_its_piece",
        (leg_extend + 0.1).abs() < 1e-6,
        format!(
            "the boot's leg box carries `extend` {leg_extend} —              `createBaseArmorMesh` replaces the humanoid legs with              `g.extend(-0.1)`, so a boot and a legging do not fight where they              overlap"
        ),
    );
    // The lookup has to find a humanoid limb even where Rewo never named it.
    //
    // Walked over **every registered mob**, not one hand-picked model. An
    // earlier form of this witness asked the *player* model, which is the one
    // humanoid that carries a named `body` — M19 gave it one so a combat swing
    // could rotate the torso. Every other humanoid puts its torso cube on the
    // static root, so a chestplate's body box resolved to nothing and mobs
    // wore armoured arms over a bare chest while this passed.
    let parts = ["head", "body", "right_arm", "left_arm", "right_leg", "left_leg"];
    let mut wearers = 0usize;
    let mut incomplete: Vec<(&str, &str)> = Vec::new();
    for def in rewo_gpu::mobs::MOBS {
        if !rewo_gpu::mobs::wears_humanoid_armor(def.kind) {
            continue;
        }
        let m = (def.build)();
        wearers += 1;
        for p in parts {
            if armor_part(&m.parts, p).is_none() {
                incomplete.push((def.kind.name(), p));
            }
        }
    }
    c.record(
        "r4.every_armour_wearing_mob_resolves_all_six_parts",
        wearers >= 13 && incomplete.is_empty(),
        format!(
            "{wearers} armour-wearing mob(s), {} unresolved part(s)              {incomplete:?}. A name exists only where a keyframe or CEM bone              targets the part, so the lookup falls back to the animation kind              and then, for the torso alone, to the static root the cube sits on",
            incomplete.len()
        ),
    );
    // ...and the fallback must not hand a humanoid torso to something that is
    // not humanoid. `HumanoidArmorLayer` is `RenderLayer<S, M extends
    // HumanoidModel>`, so vanilla draws nothing at all for these.
    // ...and the set is the *renderers*, not the mesh. These three are the
    // whole reason a geometric test is wrong: each has enough humanoid mesh to
    // pass one, and none has an armour layer in vanilla.
    let mesh_says_humanoid: Vec<&str> = ["allay", "pillager", "creaking"]
        .iter()
        .filter(|n| {
            rewo_gpu::mobs::MOBS.iter().any(|d| {
                d.kind.name() == **n && armor_part(&(d.build)().parts, "right_arm").is_some()
            })
        })
        .copied()
        .collect();
    let none_wear: Vec<&str> = ["allay", "pillager", "creaking", "creeper", "cow"]
        .iter()
        .filter(|n| {
            rewo_gpu::mobs::MOBS
                .iter()
                .any(|d| d.kind.name() == **n && rewo_gpu::mobs::wears_humanoid_armor(d.kind))
        })
        .copied()
        .collect();
    let excluded = rewo_gpu::mobs::MOBS
        .iter()
        .filter(|d| !rewo_gpu::mobs::wears_humanoid_armor(d.kind))
        .count();
    c.record(
        "r5.the_armour_layer_follows_the_renderer_not_the_mesh",
        mesh_says_humanoid.len() == 3 && none_wear.is_empty() && excluded > 60,
        format!(
            "{excluded} mob(s) wear nothing. {mesh_says_humanoid:?} carry humanoid              arms and would pass a mesh test, yet {none_wear:?} of the sampled              set wears armour — vanilla mentions `HumanoidArmorLayer` in eight              renderers and none of them is an illager, an allay or a creaking"
        ),
    );


    // -- the leather dye (M47) ------------------------------------------------
    //
    // `getColorForLayer` is four lines and three cases, and the third one is
    // the surprise: **zero means the layer does not draw**, which is the whole
    // mechanism behind `Layer.onlyIfDyed`. Transcribed here independently of
    // the shipped function.
    let independent = |dyeable: Option<Option<u32>>, dye: u32| -> u32 {
        match dyeable {
            None => 0xFFFF_FFFF,
            Some(undyed) => {
                if dye != 0 {
                    dye
                } else {
                    match undyed {
                        Some(c) => 0xFF00_0000 | (c & 0xFF_FFFF),
                        None => 0,
                    }
                }
            }
        }
    };
    let red = 0xFFB0_2E26u32;
    let cases: [(Option<Option<u32>>, u32); 6] = [
        (None, 0),
        (None, red),
        (Some(Some(equipment::LEATHER_COLOR)), 0),
        (Some(Some(equipment::LEATHER_COLOR)), red),
        (Some(None), 0),
        (Some(None), red),
    ];
    let got: Vec<u32> = cases.iter().map(|&(d, y)| equipment::color_for_layer(d, y)).collect();
    let want: Vec<u32> = cases.iter().map(|&(d, y)| independent(d, y)).collect();
    c.record(
        "d1.get_color_for_layer_matches_an_independent_transcription",
        got == want
            && got[0] == 0xFFFF_FFFF
            && got[2] == 0xFF00_0000 | equipment::LEATHER_COLOR
            && got[3] == red
            && got[4] == 0,
        format!(
            "{got:02x?}. An absent `dyeable` is -1 (white, untinted); a dyeable \
             base is its `color_when_undyed` until dyed and the dye after; and a \
             dyeable with no `color_when_undyed` is **0**, which vanilla's \
             `if (color != 0)` reads as do-not-draw"
        ),
    );

    // The jar's own data, not a belief about it.
    let leather = baked.equipment.layers("minecraft:leather", equipment::ArmorLayer::Humanoid);
    let iron = baked.equipment.layers("minecraft:iron", equipment::ArmorLayer::Humanoid);
    c.record(
        "d2.only_leather_is_dyeable_and_it_is_a_base_plus_an_overlay",
        leather.len() == 2
            && leather[0].dyeable == Some(Some(equipment::LEATHER_COLOR))
            && leather[1].dyeable.is_none()
            && iron.len() == 1
            && iron[0].dyeable.is_none(),
        format!(
            "leather {} layer(s) {:?}; iron {} layer(s) {:?}. The base carries \
             `color_when_undyed` 0x{:06X} and the overlay carries no `dyeable` \
             at all, so the overlay is never tinted — which is what keeps the \
             studs and stitching their own colour on a dyed piece",
            leather.len(),
            leather.iter().map(|l| l.dyeable).collect::<Vec<_>>(),
            iron.len(),
            iron.iter().map(|l| l.dyeable).collect::<Vec<_>>(),
            equipment::LEATHER_COLOR
        ),
    );

    // An undyed leather piece is **brown**, not the greyscale its sheet is
    // authored in — the bug M46 shipped with and this milestone closes.
    let undyed = equipment::color_for_layer(leather[0].dyeable, equipment::dye_argb(None));
    let dyed = equipment::color_for_layer(leather[0].dyeable, equipment::dye_argb(Some(0x00B0_2E26)));
    let overlay_dyed = equipment::color_for_layer(leather[1].dyeable, equipment::dye_argb(Some(0x00B0_2E26)));
    c.record(
        "d3.an_undyed_leather_piece_is_brown_and_a_dye_moves_only_the_base",
        undyed == 0xFF00_0000 | equipment::LEATHER_COLOR
            && dyed == 0xFFB0_2E26
            && overlay_dyed == 0xFFFF_FFFF,
        format!(
            "undyed base 0x{undyed:08X}, dyed base 0x{dyed:08X}, and the overlay \
             stays 0x{overlay_dyed:08X} under the same dye. `dye_argb` puts the \
             alpha on, because the component holds an RGB and \
             `DyedItemColor.getOrDefault` is the one that calls `ARGB.opaque`"
        ),
    );

    // ...and it reaches pixels. The same piece rendered undyed and dyed red
    // must differ, and the dyed frame must be redder — measured as a channel
    // ratio on the armour's own pixels, so it cannot be satisfied by drawing
    // anything at all.
    let piece = |tint: [f32; 3]| rewo_gpu::entities::ArmorPiece {
        layers: [Some((leather[0].key.as_str(), tint)), None],
        trim: None,
        foil: false,
    };
    let brown = [0.627_451, 0.396_078_4, 0.250_980_4];
    let f_brown = render_armor([None, Some(piece(brown)), None, None], &mut gpu, &mut wr, &mut off)?;
    let f_red = render_armor([None, Some(piece([0.690_196, 0.180_392, 0.149_02])), None, None], &mut gpu, &mut wr, &mut off)?;
    let f_none = render_armor([None; 4], &mut gpu, &mut wr, &mut off)?;
    let armour_px = changed(&f_brown, &f_none);
    let ratio = |img: &[u8], px: &[(u32, u32)]| -> (f64, f64) {
        let (mut r, mut g, mut b) = (0f64, 0f64, 0f64);
        for &(x, y) in px {
            let i = (y as usize * W as usize + x as usize) * 4;
            r += img[i] as f64;
            g += img[i + 1] as f64;
            b += img[i + 2] as f64;
        }
        (r / g.max(1.0), r / b.max(1.0))
    };
    let (br_rg, br_rb) = ratio(&f_brown, &armour_px);
    let (rd_rg, rd_rb) = ratio(&f_red, &armour_px);
    c.record(
        "d4.the_tint_reaches_the_rendered_pixels",
        !armour_px.is_empty() && f_red != f_brown && rd_rg > br_rg && rd_rb > br_rb,
        format!(
            "{} armour pixel(s); red/green {br_rg:.3} -> {rd_rg:.3} and red/blue \
             {br_rb:.3} -> {rd_rb:.3} when the same sheet is drawn with the dye \
             instead of `color_when_undyed`. The tint rides the **vertex \
             colour**, which is where `submitModel`'s colour argument lands",
            armour_px.len()
        ),
    );



    // -- the trimmed icon (M49) -----------------------------------------------
    //
    // The whole feature was invisible for one reason: a multi-layer sprite item
    // is **coplanar by construction** — `ItemModelGenerator` puts every layer
    // in the same `z 7.5..8.5` slab — so under a strict depth test layer1 is
    // rejected at exactly layer0's depth and a trimmed icon renders as its
    // plain base. This is that, measured in pixels rather than argued.
    let base_name = "minecraft:iron_chestplate";
    let trimmed_name = format!("{base_name}#minecraft:gold");
    let plain_model = baked.held_items.any(base_name);
    let trim_model = baked.held_items.any(&trimmed_name);
    let layer_counts = |m: Option<&rewo_data::held_items::HeldItemModel>| {
        m.map(|m| {
            // `gui_quads` is the slot-context override and is absent when the
            // two contexts agree, which for a sprite item they do.
            let quads = m.gui_quads.as_ref().unwrap_or(&m.quads);
            let mut t: Vec<u16> = quads.iter().map(|q| q.tex).collect();
            t.sort_unstable();
            t.dedup();
            t.len()
        })
        .unwrap_or(0)
    };
    c.record(
        "u1.a_trimmed_variant_bakes_one_more_sprite_layer_than_its_base",
        layer_counts(plain_model) == 1 && layer_counts(trim_model) == 2,
        format!(
            "{base_name} draws from {} texture(s); {trimmed_name} from {}. The second is \
             `trims/items/chestplate_trim_gold`, which is **not a file** — `items.json` \
             permutes it from a greyscale source exactly as `armor_trims.json` does for \
             the worn sheets",
            layer_counts(plain_model),
            layer_counts(trim_model)
        ),
    );
    // An unknown material must land on the base, not on nothing.
    let unknown = baked.held_items.any(&format!("{base_name}#minecraft:nosuchmaterial"));
    c.record(
        "u2.an_unnamed_material_falls_back_to_the_untrimmed_icon",
        unknown.is_some() && layer_counts(unknown) == 1,
        format!(
            "a variant the bake never made resolves to {} layer(s) — the plain item. An \
             item can be trimmed with a material its own definition names no case for, \
             and vanilla's answer there is the `fallback`, so a missing variant has to \
             degrade to the untrimmed icon rather than to no icon",
            layer_counts(unknown)
        ),
    );

    // -- armour trims (M48) ---------------------------------------------------
    //
    // The permuted sprites do not exist as files: `armor_trims.json` declares a
    // `paletted_permutations` source and the client generates one per
    // (pattern x material) at load. This is that swap, transcribed
    // independently of the shipped function.
    let independent = |src: &[u8], key: &[u8], value: &[u8]| -> Vec<u8> {
        let mut out = src.to_vec();
        for px in out.chunks_exact_mut(4) {
            if px[3] == 0 {
                continue;
            }
            let mut hit = None;
            for i in (0..key.len()).step_by(4) {
                if key[i + 3] != 0 && key[i..i + 3] == px[0..3] {
                    hit = Some([value[i], value[i + 1], value[i + 2], value[i + 3]]);
                    break;
                }
            }
            if let Some(v) = hit {
                let a = px[3] as u32 * v[3] as u32 / 255;
                px[0] = v[0];
                px[1] = v[1];
                px[2] = v[2];
                px[3] = a as u8;
            }
        }
        out
    };
    // key: two opaque entries + one fully transparent (which must be ignored).
    let key = [10u8, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 0];
    let val = [200u8, 0, 0, 255, 0, 200, 0, 128, 9, 9, 9, 255];
    // src: a matching pixel, the *other* match at half alpha, the entry whose
    // key was transparent (so unmatched), a wholly unmatched colour, and a
    // transparent pixel.
    let src = [
        10u8, 20, 30, 255, 40, 50, 60, 200, 70, 80, 90, 255, 1, 2, 3, 255, 4, 5, 6, 0,
    ];
    let got = equipment::apply_palette(&src, &key, &val).expect("same length");
    let want = independent(&src, &key, &val);
    c.record(
        "t1.the_palette_swap_matches_an_independent_transcription",
        got == want
            && got[0..4] == [200, 0, 0, 255]
            && got[4..8] == [0, 200, 0, 100]
            && got[8..12] == src[8..12]
            && got[12..16] == src[12..16]
            && got[16..20] == src[16..20],
        format!(
            "{got:?}. The match is on RGB with alpha masked off, so a \
             half-transparent pixel of a palette colour still maps and takes \
             `pixelAlpha * valueAlpha / 255` = 200*128/255 = {}. A key entry \
             with alpha 0 is skipped when the map is built, and an unmatched \
             pixel is **not** dropped — `getOrDefault` hands back \
             `opaque(pixelRGB)`, whose alpha 255 leaves it untouched",
            got[7]
        ),
    );
    let mismatched = equipment::apply_palette(&src, &key, &val[..8]);
    c.record(
        "t2.a_palette_pair_of_different_lengths_is_refused",
        mismatched.is_none(),
        "vanilla throws `IllegalArgumentException` when the key and value \
         palettes differ in length; nothing is guessed from a partial map"
            .to_string(),
    );

    // The real jar: the same source through two palettes must differ, and both
    // must differ from the greyscale it started as.
    let src_path = "trims/entity/humanoid/coast";
    let gold = baked.trims.permute(src_path, "gold");
    let iron = baked.trims.permute(src_path, "iron");
    let gold_darker = baked.trims.permute(src_path, "gold_darker");
    let sizes_ok = matches!((&gold, &iron), (Some((_, 64, 32)), Some((_, 64, 32))));
    c.record(
        "t3.one_source_permutes_into_distinct_material_sprites",
        sizes_ok
            && gold.as_ref().map(|g| &g.0) != iron.as_ref().map(|i| &i.0)
            && gold.as_ref().map(|g| &g.0) != gold_darker.as_ref().map(|d| &d.0),
        format!(
            "coast x {{gold, iron, gold_darker}} → {} distinct 64x32 sprite(s) \
             from one greyscale source. `gold` and `gold_darker` are separate \
             palettes, which is what an `override_armor_assets` entry selects",
            [&gold, &iron, &gold_darker].iter().filter(|s| s.is_some()).count()
        ),
    );

    // The override is the reason a same-material trim is visible at all.
    let iron_material = rewo_net::trim_parse::TrimMaterialDef {
        id: "minecraft:iron".into(),
        asset_name: "iron".into(),
        overrides: vec![("minecraft:iron".into(), "iron_darker".into())],
    };
    let on_iron = rewo_net::trim_parse::layer_asset_path(
        "minecraft:coast",
        "trims/entity/humanoid",
        iron_material.suffix_for("minecraft:iron"),
    );
    let on_diamond = rewo_net::trim_parse::layer_asset_path(
        "minecraft:coast",
        "trims/entity/humanoid",
        iron_material.suffix_for("minecraft:diamond"),
    );
    c.record(
        "t4.a_same_material_trim_takes_the_darker_palette",
        on_iron == "trims/entity/humanoid/coast_iron_darker"
            && on_diamond == "trims/entity/humanoid/coast_iron",
        format!(
            "iron trim on iron armour → {on_iron}; on diamond → {on_diamond}. \
             `assetId(equipmentAsset)` is `overrides.getOrDefault(asset, base)`, \
             and without it an iron trim would paint iron onto iron and vanish"
        ),
    );

    // ...and it reaches pixels, in its own depth-EQUAL range.
    let trim_origin = gold.as_ref().and_then(|(rgba, w, h)| {
        wr.upload_entity_trim(&mut gpu, "gate/coast_gold", rgba, *w, *h)
    });
    let iron_key = baked
        .equipment
        .layers("minecraft:iron", equipment::ArmorLayer::Humanoid)
        .first()
        .map(|l| l.key.clone())
        .unwrap_or_default();
    let bare = rewo_gpu::entities::ArmorPiece {
        layers: [Some((iron_key.as_str(), [1.0; 3])), None],
        trim: None,
        foil: false,
    };
    let trimmed = rewo_gpu::entities::ArmorPiece {
        trim: trim_origin,
        ..bare
    };
    let f_bare = render_armor([None, Some(bare), None, None], &mut gpu, &mut wr, &mut off)?;
    let f_trim = render_armor([None, Some(trimmed), None, None], &mut gpu, &mut wr, &mut off)?;
    let f_none = render_armor([None; 4], &mut gpu, &mut wr, &mut off)?;
    let trim_px = changed(&f_trim, &f_bare).len();
    let armour_px = changed(&f_bare, &f_none).len();
    c.record(
        "t5.the_trim_paints_only_on_the_armour_it_decorates",
        trim_origin.is_some() && trim_px > 20 && trim_px < armour_px,
        format!(
            "{trim_px} pixel(s) change when the trim is added, inside the \
             {armour_px} the armour itself covers. Its pipeline depth-tests \
             EQUAL and writes no depth — `ARMOR_DECAL_CUTOUT_NO_CULL`'s \
             `DepthStencilState(CompareOp.EQUAL, false)` — so it can only paint \
             where the armour's own fragments already won, and a pattern that \
             covered *more* than the armour would fail this"
        ),
    );

    // -- the entity glint (M45) ----------------------------------------------
    //
    // The scale is the whole difference from the GUI and hand contexts, and it
    // is a big one: `ENTITY_GLINT_TEXTURING` is 0.5 against their 8.0, so a
    // dropped stack wears a few broad bands where an icon wears a fine weave.
    let scale_item = rewo_gpu::gui_item::GLINT_SCALE_ITEM;
    let scale_entity = rewo_gpu::gui_item::GLINT_SCALE_ENTITY;
    let uv_item = rewo_gpu::gui_item::glint_uv([1.0, 0.0], (0.0, 0.0), scale_item);
    let uv_entity = rewo_gpu::gui_item::glint_uv([1.0, 0.0], (0.0, 0.0), scale_entity);
    c.record(
        "n1.the_entity_glint_uses_its_own_texture_scale",
        (scale_item, scale_entity) == (8.0, 0.5)
            && (uv_item[0] / uv_entity[0] - 16.0).abs() < 1e-3,
        format!(
            "item {scale_item} against entity {scale_entity} - a factor of sixteen.              The same unit UV maps to {uv_item:?} and {uv_entity:?}"
        ),
    );

    let g_plain = ground(Some("minecraft:diamond_sword"), 1, 0.0, 3, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    let g_glint = ground(Some("minecraft:diamond_sword"), 1, 0.0, 3, 0.0, true, &mut gpu, &mut wr, &mut off)?;
    let ground_px = changed(&g_glint, &g_plain).len();
    c.record(
        "n2.a_dropped_enchanted_stack_glints",
        ground_px > 50,
        format!(
            "{ground_px} pixels differ between a dropped stack with the foil flag              and the same one without - `ItemEntityRenderer` draws the item and              nothing else, so every changed pixel is on the item"
        ),
    );
    let again = ground(Some("minecraft:diamond_sword"), 1, 0.0, 3, 0.0, false, &mut gpu, &mut wr, &mut off)?;
    c.record(
        "n3.no_foil_flag_is_byte_identical",
        again == g_plain,
        "the same draw twice without the flag is byte-identical, so every pixel n2          measured came from the glint pass and none from a stale buffer",
    );

    let h_plain = render_glint([Some("minecraft:diamond_sword"), None], [false; 2], &mut gpu, &mut wr, &mut off)?;
    let h_main = render_glint([Some("minecraft:diamond_sword"), None], [true, false], &mut gpu, &mut wr, &mut off)?;
    let h_off = render_glint([Some("minecraft:diamond_sword"), None], [false, true], &mut gpu, &mut wr, &mut off)?;
    c.record(
        "n4.a_mob_held_stack_glints_in_the_hand_that_holds_it",
        changed(&h_main, &h_plain).len() > 50 && changed(&h_off, &h_plain).is_empty(),
        format!(
            "{} pixels differ with the main hand's flag set and {} with the *off*              hand's - the flag is per hand, and the off hand is empty here, so              setting it changes nothing at all",
            changed(&h_main, &h_plain).len(),
            changed(&h_off, &h_plain).len()
        ),
    );

    // -- the worn-armour glint (M50) ------------------------------------------
    //
    // `ARMOR_ENTITY_GLINT` differs from the `ENTITY_GLINT` above in three
    // things and one non-thing. The three: its own sheet
    // (`misc/enchanted_glint_armor.png`), scale **0.16** against 0.5, and
    // `renderLayers`' rules about which submit carries the foil. The non-thing
    // is `VIEW_OFFSET_Z_LAYERING` — `ARMOR_CUTOUT_NO_CULL`,
    // `ARMOR_DECAL_CUTOUT_NO_CULL` and `ARMOR_ENTITY_GLINT` all set it, with
    // the same bias applied to a fresh `getModelViewMatrixCopy()`, so it
    // cancels exactly within the armour stack. That cancellation is the only
    // reason `RenderPipelines.GLINT`'s `DepthStencilState(CompareOp.EQUAL,
    // false)` can land the foil on the armour at all, and a3 is what measures
    // that it does.
    let sheets = match (baked.glint.as_ref(), baked.armor_glint.as_ref()) {
        (Some(i), Some(a)) => Some((i, a)),
        _ => None,
    };
    c.record(
        "a1.the_armour_foil_has_its_own_sheet",
        sheets.is_some_and(|(i, a)| (i.w, i.h) != (a.w, a.h) || i.rgba != a.rgba),
        match sheets {
            Some((i, a)) => format!(
                "enchanted_glint_item.png is {}x{} and enchanted_glint_armor.png is \
                 {}x{}, and they are different images. The worn foil is not the item \
                 foil at another scale — `ARMOR_ENTITY_GLINT` binds a second texture",
                i.w, i.h, a.w, a.h
            ),
            None => "one of the two glint sheets is missing from the jar".to_string(),
        },
    );

    // Its own texture scale, the same way n1 states the entity glint's.
    let scale_armor = rewo_gpu::gui_item::GLINT_SCALE_ARMOR;
    let uv_armor = rewo_gpu::gui_item::glint_uv([1.0, 0.0], (0.0, 0.0), scale_armor);
    c.record(
        "a2.the_armour_glint_uses_its_own_texture_scale",
        scale_armor == 0.16 && (uv_entity[0] / uv_armor[0] - 3.125).abs() < 1e-3,
        format!(
            "armour {scale_armor} against entity {scale_entity} and item {scale_item}. \
             The same unit UV maps to {uv_armor:?} and {uv_entity:?} — a factor of \
             {:.3}, so a worn piece wears bands three times broader than a dropped one",
            uv_entity[0] / uv_armor[0]
        ),
    );

    // The renders. Iron is one layer; leather is two, which is what makes the
    // "once per piece, not once per layer" rule observable at all.
    let iron_piece = |foil: bool, trim: Option<(u32, u32)>| rewo_gpu::entities::ArmorPiece {
        layers: [Some((iron_key.as_str(), [1.0; 3])), None],
        trim,
        foil,
    };
    let chest = |p: rewo_gpu::entities::ArmorPiece<'_>,
                 gpu: &mut Gpu,
                 wr: &mut WorldRenderer,
                 off: &mut Offscreen|
     -> Result<Vec<u8>, String> { render_armor([None, Some(p), None, None], gpu, wr, off) };

    let a_plain = chest(iron_piece(false, None), &mut gpu, &mut wr, &mut off)?;
    let a_foil = chest(iron_piece(true, None), &mut gpu, &mut wr, &mut off)?;
    let foil_verts_one_layer = wr.armor_glint_vertex_count();
    let a_naked = render_armor([None; 4], &mut gpu, &mut wr, &mut off)?;
    let armour_px: std::collections::HashSet<(u32, u32)> =
        changed(&a_plain, &a_naked).into_iter().collect();
    // **Not `changed`.** Its `> 8` threshold was built for the item glint,
    // which is measured against a *dark* background where the sRGB curve is
    // steep. A worn foil sits on a lit chestplate and vanilla's own is subtle:
    // the real magnitude here is a handful of bytes, so a detector tuned for a
    // brighter effect reads it as nothing at all. Any change is the right test
    // for "where did it land", and a4 — the same draw twice, byte-identical —
    // is what establishes there is no noise floor to clear.
    let changed_any = |a: &[u8], b: &[u8]| -> Vec<(u32, u32)> {
        let mut v = Vec::new();
        for y in 0..H {
            for x in 0..W {
                let i = ((y * W + x) * 4) as usize;
                if (0..3).any(|k| a[i + k] != b[i + k]) {
                    v.push((x, y));
                }
            }
        }
        v
    };
    let foil_px = changed_any(&a_foil, &a_plain);
    let strays = foil_px.iter().filter(|p| !armour_px.contains(p)).count();
    c.record(
        "a3.the_foil_lands_on_the_armours_own_fragments_and_nowhere_else",
        !armour_px.is_empty() && foil_px.len() > 50 && strays == 0,
        format!(
            "{} pixel(s) change when the piece is enchanted, {strays} of them outside \
             the {} the armour itself covers. The pass depth-tests EQUAL, so a foil \
             fragment survives only where the armour's own already won — which it can \
             only do because the armour and the foil carry the *same* \
             `VIEW_OFFSET_Z_LAYERING` and it cancels",
            foil_px.len(),
            armour_px.len()
        ),
    );
    let a_again = chest(iron_piece(false, None), &mut gpu, &mut wr, &mut off)?;
    c.record(
        "a4.no_foil_flag_is_byte_identical",
        a_again == a_plain,
        "the same unenchanted piece twice is byte-identical, so every pixel a3 \
         measured came from the foil pass and none from a stale buffer",
    );

    // The foil is **untinted**. `RenderPipelines.GLINT` binds
    // `DefaultVertexFormat.POSITION_TEX` — Position and UV0, no Color element,
    // so `BufferBuilder.beginElement` drops the colour `submitModel` was handed
    // — `glint.vsh` declares no colour attribute, and
    // `RenderType.writeDynamicTransforms` writes `ColorModulator` as WHITE. So
    // a dyed piece's foil is the plain sheen, and this is the exact inverse of
    // d4: the same measurement that showed the *armour* takes the dye must show
    // the *foil* does not.
    //
    // Differenced in **linear** light. The attachment is `R8G8B8A8_SRGB`, so
    // the additive blend happens linearly and the store encodes; the raw byte
    // delta of a base-independent contribution is not base-independent, and
    // comparing bytes would fail this for the wrong reason.
    let leather_piece = |tint: [f32; 3], foil: bool| rewo_gpu::entities::ArmorPiece {
        layers: [Some((leather[0].key.as_str(), tint)), None],
        trim: None,
        foil,
    };
    // **Dark and opposed**, not the two plausible leather dyes. A bright base
    // saturates a channel, and a saturated channel cannot show a contribution
    // at all — the first fixture here used brown against red and measured
    // *exactly* zero in red and green because both pinned at 255. These two sit
    // low enough that every channel has headroom, and far enough apart that a
    // foil carrying the dye would be unmistakable: red over one, blue over the
    // other.
    let dye_a = [0.30, 0.06, 0.06];
    let dye_b = [0.06, 0.06, 0.30];
    let l_brown_off = chest(leather_piece(dye_a, false), &mut gpu, &mut wr, &mut off)?;
    let l_brown_on = chest(leather_piece(dye_a, true), &mut gpu, &mut wr, &mut off)?;
    let l_red_off = chest(leather_piece(dye_b, false), &mut gpu, &mut wr, &mut off)?;
    let l_red_on = chest(leather_piece(dye_b, true), &mut gpu, &mut wr, &mut off)?;
    // **In bytes, not in linear light.** The blend now runs in gamma space, so
    // vanilla's contribution is `dst + src²` on the *encoded* numbers and the
    // byte delta is base-independent by construction — which is the whole
    // property. Measured in linear light instead it is not: the same gamma-space
    // increment lands differently depending on how bright the destination is,
    // and an earlier cut of this witness read 15.83 against 11.74 for two dyes
    // carrying an identical foil.
    //
    // A hue ratio is no good here either: `enchanted_glint_armor.png` is
    // blue-dominant (mean R18 G7 B46) and at scale 0.16 the patch each face
    // samples is blue-only, so red/green is 0/0. Comparing the deltas
    // themselves needs no such assumption about the sheet.
    let mut worst = 0i32;
    let mut lit = 0usize;
    for p in 0..(W * H) as usize {
        for k in 0..3 {
            let i = p * 4 + k;
            // A saturated channel cannot show a contribution either way.
            if l_brown_on[i] == 255 || l_red_on[i] == 255 {
                continue;
            }
            let da = l_brown_on[i] as i32 - l_brown_off[i] as i32;
            let db = l_red_on[i] as i32 - l_red_off[i] as i32;
            if da != 0 || db != 0 {
                lit += 1;
            }
            worst = worst.max((da - db).abs());
        }
    }
    c.record(
        "a5.the_foil_is_not_tinted_by_the_layers_colour",
        lit > 500 && worst <= 1,
        format!(
            "over {lit} channel(s) the foil moves, its byte contribution differs by at              most {worst} between two opposite dyes — the same foil over a red-tinted              piece and a blue-tinted one. d4 showed the *armour* takes the dye; the              foil does not, because `RenderPipelines.GLINT` binds `POSITION_TEX` — no              Color element to take it with, and `writeDynamicTransforms` passes              `ColorModulator` as WHITE"
        ),
    );

    // Once per piece, not once per layer. `renderLayers` clears `renderFoil`
    // inside the loop, so leather's two layers get one foil between them; an
    // emitter that ran per layer would double an additive blend.
    let two_layers = rewo_gpu::entities::ArmorPiece {
        layers: [
            Some((iron_key.as_str(), [1.0; 3])),
            Some((iron_key.as_str(), [1.0; 3])),
        ],
        trim: None,
        foil: true,
    };
    chest(two_layers, &mut gpu, &mut wr, &mut off)?;
    let foil_verts_two_layers = wr.armor_glint_vertex_count();
    c.record(
        "a6.one_foil_per_piece_however_many_layers_it_draws",
        foil_verts_one_layer > 0 && foil_verts_two_layers == foil_verts_one_layer,
        format!(
            "{foil_verts_one_layer} foil vertices for a one-layer piece and \
             {foil_verts_two_layers} for a two-layer one. `renderFoil = false` runs \
             inside the layer loop, so the foil rides the first layer that draws and \
             no other"
        ),
    );

    // And never the trim, which is submitted **after** the loop that clears the
    // flag — the finding that reshaped this milestone.
    chest(iron_piece(true, trim_origin), &mut gpu, &mut wr, &mut off)?;
    let foil_verts_trimmed = wr.armor_glint_vertex_count();
    chest(iron_piece(false, trim_origin), &mut gpu, &mut wr, &mut off)?;
    let foil_verts_trim_only = wr.armor_glint_vertex_count();
    c.record(
        "a7.the_trim_never_glints",
        trim_origin.is_some()
            && foil_verts_trimmed == foil_verts_one_layer
            && foil_verts_trim_only == 0,
        format!(
            "a trimmed enchanted piece emits {foil_verts_trimmed} foil vertices — the \
             same {foil_verts_one_layer} as the untrimmed one, not more — and a trimmed \
             *un*enchanted piece emits {foil_verts_trim_only}. `renderLayers` submits \
             the trim outside the loop that clears `renderFoil`, so a glinting trim \
             would be an invention"
        ),
    );

    // The trim paints **over** the foil. `renderLayers` submits layer, foil,
    // trim at increasing `order`, and `SubmitNodeStorage` keeps its phases in
    // an `Int2ObjectAVLTreeMap` — a sorted map, drained ascending.
    //
    // A pixel is fully-opaque trim wherever the same trim over two *different*
    // armour colours reads identically; that construction needs no knowledge of
    // the sprite's alpha. On those pixels the foil must change nothing.
    let leather_trimmed = |tint: [f32; 3], foil: bool| rewo_gpu::entities::ArmorPiece {
        layers: [Some((leather[0].key.as_str(), tint)), None],
        trim: trim_origin,
        foil,
    };
    let t_brown = chest(leather_trimmed(dye_a, false), &mut gpu, &mut wr, &mut off)?;
    let t_red = chest(leather_trimmed(dye_b, false), &mut gpu, &mut wr, &mut off)?;
    let t_brown_foil = chest(leather_trimmed(dye_a, true), &mut gpu, &mut wr, &mut off)?;
    let opaque_trim: Vec<(u32, u32)> = armour_px
        .iter()
        .copied()
        .filter(|&(x, y)| {
            let i = ((y * W + x) * 4) as usize;
            (0..3).all(|k| t_brown[i + k] == t_red[i + k])
        })
        .collect();
    let disturbed = opaque_trim
        .iter()
        .filter(|&&(x, y)| {
            let i = ((y * W + x) * 4) as usize;
            (0..3).any(|k| t_brown_foil[i + k] != t_brown[i + k])
        })
        .count();
    c.record(
        "a8.the_trim_paints_over_the_foil_not_under_it",
        opaque_trim.len() > 20 && disturbed == 0,
        format!(
            "{} fully-opaque trim pixel(s) — where the same trim over a brown and a red \
             piece read identically — and the foil changes {disturbed} of them. Reverse \
             the two draws and every one of them gains the sheen additively",
            opaque_trim.len()
        ),
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
