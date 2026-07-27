//! `rewo inventoryshot --check` — the inventory and hotbar-icon oracle (M34).
//!
//! Serverless, validation-required, fail-closed. Three layers, the shape the
//! other gates use:
//!
//! 1. **The wire**, driven through the production `route_inventory` — the three
//!    packet ids resolved by name, the two coordinate systems, and the rules
//!    that decide what a malformed or foreign packet is allowed to change.
//! 2. **The placement**, on the CPU — where a sprite and a block land in a
//!    slot, which is where the two `display.gui` cases stop being
//!    interchangeable.
//! 3. **The pixels**, by rendering real baked items into real hotbar slots
//!    through the production pass and reading them back.
//!
//! The packets are synthesised here rather than captured, because the point of
//! the first layer is to drive the *decoder*. The items and their models come
//! from the jar bake, because the point of the third is to draw what the client
//! would.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use rewo_data::{assets, packets::Packets, DataPaths};
use rewo_gpu::gui_item::{
    build_vertices, direction_normal, place, GuiItem, ItemLights, PX_PER_BLOCK,
};
use rewo_gpu::held::DisplayTransform;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_world::inventory::{Inventory, ItemSlot, HOTBAR_MENU_START, MENU_SLOTS};

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 16;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 256;
const H: u32 = 256;

#[derive(ClapArgs, Debug)]
pub struct InventoryshotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[inventoryshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: InventoryshotArgs) -> Result<(), String> {
    println!("[inventoryshot] the oracle asserts unconditionally (--check selects the exit code)");
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir for version data")?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_wire(&mut c, &paths)?;
    check_placement(&mut c);
    check_pixels(&mut c, &baked, &args)?;

    println!(
        "[inventoryshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
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
    println!("[inventoryshot] PASS — {} witnesses", c.witnessed);
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

// -- 1. the wire ---------------------------------------------------------------

fn push_varint(v: &mut Vec<u8>, n: i32) {
    let mut u = n as u32;
    loop {
        let b = (u & 0x7F) as u8;
        u >>= 7;
        if u == 0 {
            v.push(b);
            return;
        }
        v.push(b | 0x80);
    }
}

/// `ItemStack.OPTIONAL_STREAM_CODEC` with an empty component patch.
fn stack_bytes(item_id: i32, count: i32) -> Vec<u8> {
    let mut v = Vec::new();
    push_varint(&mut v, count);
    if count > 0 {
        push_varint(&mut v, item_id);
        push_varint(&mut v, 0); // components added
        push_varint(&mut v, 0); // components removed
    }
    v
}

fn check_wire(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    let packets = Packets::load(&paths.packets_json())?;
    let ids = rewo_net::ids::Ids::resolve(&packets)?;
    let components = rewo_data::components::DataComponentIds::load(&paths.registries_json())
        .map_err(|e| format!("component ids: {e}"))?;
    let route = |id: i32, body: &[u8], inv: &mut Inventory| -> bool {
        rewo_net::route_inventory(id, body, &ids, Some(components), inv)
    };

    c.record(
        "w1.the_three_ids_resolve_by_name_and_differ",
        ids.cb_play_container_set_content != ids.cb_play_container_set_slot
            && ids.cb_play_container_set_slot != ids.cb_play_set_held_slot,
        format!(
            "content {}, slot {}, held {} — resolved from the report by name, so a \
             renumbered protocol fails loud rather than writing an item into the \
             wrong place",
            ids.cb_play_container_set_content,
            ids.cb_play_container_set_slot,
            ids.cb_play_set_held_slot
        ),
    );

    // A full container: 46 menu slots, something in the third hotbar one.
    let mut body = Vec::new();
    push_varint(&mut body, 0); // containerId — the player's own inventory
    push_varint(&mut body, 7); // stateId
    push_varint(&mut body, MENU_SLOTS as i32);
    for i in 0..MENU_SLOTS {
        let carry = i == HOTBAR_MENU_START + 2;
        body.extend_from_slice(&stack_bytes(if carry { 276 } else { 0 }, if carry { 5 } else { 0 }));
    }
    body.extend_from_slice(&stack_bytes(0, 0)); // the carried stack

    let mut inv = Inventory::default();
    let matched = route(ids.cb_play_container_set_content, &body, &mut inv);
    c.record(
        "w2.a_full_container_reaches_the_hotbar_through_the_real_route",
        matched
            && inv.hotbar(2)
                == Some(ItemSlot {
                    item_id: 276,
                    count: 5,
                })
            && inv.hotbar(0).is_none(),
        format!(
            "hotbar 2 is {:?}, hotbar 0 empty. Menu slot {} is hotbar index 2 — the \
             conversion between the wire's 46-slot menu and the 9 slots the game \
             logic indexes, and the reason a pickaxe lands in a hotbar rather than \
             an armour slot",
            inv.hotbar(2),
            HOTBAR_MENU_START + 2
        ),
    );

    // The selection is an INVENTORY index, and out of range is ignored.
    let mut good = Vec::new();
    push_varint(&mut good, 2);
    route(ids.cb_play_set_held_slot, &good, &mut inv);
    let selected = inv.selected();
    let holding = inv.held();
    let mut bad = Vec::new();
    push_varint(&mut bad, 40);
    route(ids.cb_play_set_held_slot, &bad, &mut inv);
    c.record(
        "w3.an_out_of_range_held_slot_is_ignored_not_clamped",
        selected == 2 && inv.selected() == 2 && holding.is_some(),
        format!(
            "slot 2 selected, then slot 40 left it at {}. Vanilla's \
             `if (Inventory.isHotbarSlot(...))` guard neither clamps nor resets, so a \
             bad value keeps your selection rather than silently moving it to 0 or 8",
            inv.selected()
        ),
    );

    // A single slot, whose index is an i16 where its neighbours use var-ints.
    let mut one = Vec::new();
    push_varint(&mut one, 0);
    push_varint(&mut one, 8);
    one.extend_from_slice(&((HOTBAR_MENU_START + 2) as i16).to_be_bytes());
    one.extend_from_slice(&stack_bytes(64, 1));
    route(ids.cb_play_container_set_slot, &one, &mut inv);
    c.record(
        "w4.a_single_slot_update_reads_its_index_as_a_short",
        inv.hotbar(2).map(|s| s.item_id) == Some(64),
        format!(
            "the slot became {:?}. Reading that index as a var-int would consume one \
             byte where the wire spends two, and every field after it would be \
             misaligned",
            inv.hotbar(2)
        ),
    );

    // A container id other than 0 belongs to an open screen this client has not
    // got.
    let mut foreign = Vec::new();
    push_varint(&mut foreign, 3);
    push_varint(&mut foreign, 9);
    foreign.extend_from_slice(&0i16.to_be_bytes());
    foreign.extend_from_slice(&stack_bytes(1, 1));
    let before = inv.menu_slot(0);
    route(ids.cb_play_container_set_slot, &foreign, &mut inv);
    c.record(
        "w5.another_containers_update_is_ignored",
        inv.menu_slot(0) == before,
        "every container id but 0 is an open chest or crafting screen, and Rewo has \
         none — so leaving the player inventory alone is the whole truth about what \
         this client can show",
    );

    // A list that runs out mid-way must abandon the packet whole.
    let mut trunc = Vec::new();
    push_varint(&mut trunc, 0);
    push_varint(&mut trunc, 11);
    push_varint(&mut trunc, MENU_SLOTS as i32);
    trunc.extend_from_slice(&stack_bytes(999, 1)); // one slot, then nothing
    let snapshot = inv.hotbar(2);
    route(ids.cb_play_container_set_content, &trunc, &mut inv);
    c.record(
        "w6.a_truncated_container_leaves_the_previous_contents_standing",
        inv.hotbar(2) == snapshot,
        format!(
            "the hotbar is still {:?}. Once the list runs short every following slot \
             is garbage; half-applying would show a confident, wrong inventory, \
             which is worse than showing the last good one",
            inv.hotbar(2)
        ),
    );
    Ok(())
}

// -- 2. the placement ----------------------------------------------------------

fn check_placement(c: &mut Checker) {
    let centre = [100.0f32, 200.0];
    let sprite = DisplayTransform::default();
    let a = place(&sprite, [0.0, 0.0, 0.0], centre, PX_PER_BLOCK);
    let b = place(&sprite, [16.0, 16.0, 0.0], centre, PX_PER_BLOCK);
    c.record(
        "p1.a_sprite_fills_its_slot_exactly",
        (a[0] - (centre[0] - 8.0)).abs() < 1e-4
            && (b[0] - (centre[0] + 8.0)).abs() < 1e-4
            && (a[1] - (centre[1] + 8.0)).abs() < 1e-4,
        "an identity `display.gui` maps 0..16 model units onto -8..8 px — exactly the \
         16 px slot. That is why an ABSENT gui transform is *correct* for a sprite \
         rather than missing, and why inheriting the block one would tilt every sword \
         in the hotbar",
    );

    let block = DisplayTransform {
        rotation: [30.0, 225.0, 0.0],
        translation: [0.0; 3],
        scale: [0.625; 3],
    };
    let mut reach = 0.0f32;
    for &x in &[0.0f32, 16.0] {
        for &y in &[0.0f32, 16.0] {
            for &z in &[0.0f32, 16.0] {
                let p = place(&block, [x, y, z], [0.0, 0.0], PX_PER_BLOCK);
                reach = reach.max(p[0].abs()).max(p[1].abs());
            }
        }
    }
    c.record(
        "p2.a_block_tilts_and_slightly_overflows_its_slot",
        reach > 8.0 && reach < 10.0,
        format!(
            "a block's corners reach {reach:.2} px against the slot's 8 px half-width \
             — `scale 0.625` with `rotation [30, 225, 0]` overflows a little, as \
             vanilla's does"
        ),
    );

    let lights = ItemLights::default();
    c.record(
        "p3.the_flat_and_3d_lighting_poses_differ",
        (lights.flat.0 - lights.three_d.0).length() > 0.1
            && [
                lights.flat.0,
                lights.flat.1,
                lights.three_d.0,
                lights.three_d.1,
            ]
            .iter()
            .all(|v| (v.length() - 1.0).abs() < 1e-5),
        format!(
            "ITEMS_FLAT {:?} against ITEMS_3D {:?}, both unit length. A block in the \
             hotbar is lit from a different angle than a flat sprite, and both differ \
             from the Direction-ordinal shade Rewo's world and hand passes use",
            lights.flat.0, lights.three_d.0
        ),
    );

    let up = lights.shade(direction_normal(1), true);
    let down = lights.shade(direction_normal(0), true);
    c.record(
        "p4.the_diffuse_is_bounded_and_direction_dependent",
        (0.4..=1.0).contains(&up) && (0.4..=1.0).contains(&down) && up != down,
        format!(
            "up {up:.3}, down {down:.3} — `min(1, (max(0,n·L0) + max(0,n·L1)) * 0.6 + \
             0.4)`, so no face is ever black and none blows out, but the two are not \
             the same either"
        ),
    );
}

// -- 3. the pixels -------------------------------------------------------------

/// Count pixels that differ between two frames, in a rectangle.
///
/// A difference against the same scene without the item, rather than a count of
/// non-black pixels: the world pass paints a sky behind everything, so "not the
/// clear colour" would be true of every pixel in both frames and measure
/// nothing.
fn changed(a: &[u8], b: &[u8], x: u32, y: u32, w: u32, h: u32) -> i64 {
    let mut n = 0;
    for yy in y..(y + h).min(H) {
        for xx in x..(x + w).min(W) {
            let i = ((yy * W + xx) * 4) as usize;
            if a[i..i + 3] != b[i..i + 3] {
                n += 1;
            }
        }
    }
    n
}

/// The bounding box of the pixels that differ, as `(x0, y0, x1, y1)` exclusive,
/// or `None` if nothing changed.
fn changed_bounds(a: &[u8], b: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (W, H, 0u32, 0u32);
    for yy in 0..H {
        for xx in 0..W {
            let i = ((yy * W + xx) * 4) as usize;
            if a[i..i + 3] != b[i..i + 3] {
                x0 = x0.min(xx);
                y0 = y0.min(yy);
                x1 = x1.max(xx + 1);
                y1 = y1.max(yy + 1);
            }
        }
    }
    (x1 > x0).then_some((x0, y0, x1, y1))
}

fn check_pixels(
    c: &mut Checker,
    baked: &assets::BakedAssets,
    args: &InventoryshotArgs,
) -> Result<(), String> {
    let held = crate::live_cmd::to_gpu_held_items(&baked.held_items);
    let lights = ItemLights::default();

    // One item from each geometry path M22 built.
    let sprite = "minecraft:diamond_sword";
    let block = "minecraft:dirt";
    c.record(
        "g1.both_geometry_paths_are_available_to_grade",
        held.any(sprite)
            .is_some_and(|m| !m.from_block && !m.quads.is_empty())
            && held
                .any(block)
                .is_some_and(|m| m.from_block && !m.quads.is_empty()),
        format!(
            "{sprite}: {} quads, from_block {}. {block}: {} quads, from_block {}",
            held.any(sprite).map_or(0, |m| m.quads.len()),
            held.any(sprite).is_some_and(|m| m.from_block),
            held.any(block).map_or(0, |m| m.quads.len()),
            held.any(block).is_some_and(|m| m.from_block),
        ),
    );

    // Two well-separated slots, so each can be measured on its own.
    let slots = [
        GuiItem {
            model: sprite.into(),
            x: 32.0,
            y: 96.0,
            size: 48.0,
        },
        GuiItem {
            model: block.into(),
            x: 144.0,
            y: 96.0,
            size: 48.0,
        },
    ];
    let names: Vec<String> = slots.iter().map(|s| s.model.clone()).collect();
    let wanted = crate::live_cmd::gui_atlas_wanted(&held, &names);
    let atlas = crate::live_cmd::pack_gui_atlas(&held, &wanted);
    let uv = atlas.uv.clone();
    let verts = build_vertices(&held, &slots, &lights, &|t| uv.get(&t).copied());

    // An unbaked model contributes nothing rather than something wrong — the
    // same rule the item path uses for the 147 state-dependent definitions M22
    // suppresses.
    let bogus = [GuiItem {
        model: "minecraft:definitely_not_an_item".into(),
        x: 32.0,
        y: 96.0,
        size: 48.0,
    }];
    c.record(
        "g2.an_unbaked_model_contributes_nothing",
        build_vertices(&held, &bogus, &lights, &|t| uv.get(&t).copied()).is_empty(),
        "no vertices at all, rather than a default cube or a stretched atlas corner",
    );

    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[inventoryshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("inventoryshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    let r = pixels_inner(
        c, &mut gpu, &mut off, &mut wr, baked, &atlas, &verts, &draw, args,
    );
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    r
}

#[allow(clippy::too_many_arguments)]
fn pixels_inner(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    baked: &assets::BakedAssets,
    atlas: &crate::live_cmd::GuiAtlas,
    verts: &[rewo_gpu::gui_item::GuiItemVertex],
    draw: &OverlayDraw,
    args: &InventoryshotArgs,
) -> Result<(), String> {
    // The icons draw on top of the hotbar the HUD paints, so the HUD has to
    // exist for them to appear at all — which is itself the M34 wiring claim.
    let sprites = crate::live_cmd::hud_sprites(baked).ok_or("hud sprites missing from the jar")?;
    wr.init_hud(gpu, &sprites)?;
    wr.set_hud(20.0, 20, 0);
    wr.init_gui_items(gpu, &atlas.rgba, 512, 512)?;

    // The GUI pass works in screen pixels and ignores the world's matrix
    // entirely, so any view-projection will do.
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    v: &[rewo_gpu::gui_item::GuiItemVertex]|
     -> Result<Vec<u8>, String> {
        wr.set_gui_items(gpu, v)?;
        off.render(gpu, Some((&mut *wr, vp)), draw, CLEAR)?;
        off.read_rgba(gpu)
    };

    let empty = shot(gpu, off, wr, &[])?;
    let img = shot(gpu, off, wr, verts)?;
    if let Some(d) = &args.out_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("out-dir: {e}"))?;
        let _ = off.save_png(gpu, &d.join("inventoryshot.png"));
    }

    // Measure each slot as a difference against the same scene without items,
    // so the sky behind and the hotbar frame below cannot be mistaken for an
    // icon.
    let a = changed(&img, &empty, 32, 96, 48, 48);
    let b = changed(&img, &empty, 144, 96, 48, 48);
    c.record(
        "g3.both_items_render_in_their_own_slots",
        a > 200 && b > 200,
        format!("sprite slot changed {a} px, block slot changed {b} px"),
    );

    let gap = changed(&img, &empty, 88, 96, 48, 48);
    c.record(
        "g4.the_slot_origin_is_honoured",
        gap == 0,
        format!(
            "{gap} px changed between the two slots — an item placed at a fixed \
             point rather than its own `GuiItem::x/y` would light this up"
        ),
    );

    // The mutation partner for the whole placement layer: the same block, drawn
    // with an identity `display.gui`, must land differently. This is what makes
    // "the gui transform is applied, and it is the block one" falsifiable —
    // every other pixel witness would pass just as happily on an item drawn
    // face-on.
    let mut flat = crate::live_cmd::to_gpu_held_items(&baked.held_items);
    if let Some(m) = flat.models.get_mut("minecraft:dirt") {
        m.gui = DisplayTransform::default();
    }
    let flat_slots = [GuiItem {
        model: "minecraft:dirt".into(),
        x: 144.0,
        y: 96.0,
        size: 48.0,
    }];
    let lights = ItemLights::default();
    let flat_verts = build_vertices(&flat, &flat_slots, &lights, &|t| {
        atlas.uv.get(&t).copied()
    });
    let flat_img = shot(gpu, off, wr, &flat_verts)?;
    let flat_box = changed_bounds(&flat_img, &empty);
    let tilt_box = changed_bounds(&img, &empty).map(|(x0, y0, x1, y1)| {
        // The full frame has both items; take the block's half.
        (x0.max(112), y0, x1, y1)
    });
    let flat_fills = flat_box.is_some_and(|(x0, y0, x1, y1)| {
        // Face-on and unscaled, the cube covers its slot exactly.
        x0 == 144 && y0 == 96 && x1 == 192 && y1 == 144
    });
    c.record(
        "g5.the_blocks_gui_transform_is_applied_and_is_the_block_one",
        flat_fills && flat_box != tilt_box,
        format!(
            "identity {flat_box:?} against the baked transform {tilt_box:?}. Face-on \
             and unscaled the cube covers its 48 px slot exactly; `scale 0.625` with \
             `rotation [30, 225, 0]` puts it somewhere else entirely. Substituting \
             one transform for the other is the mutation this rejects"
        ),
    );

    // The sensitivity partner for all three: with no items the frame is exactly
    // what the HUD alone draws.
    let again = shot(gpu, off, wr, &[])?;
    c.record(
        "g6.no_items_is_byte_identical_to_the_hud_alone",
        again == empty && img != empty,
        format!(
            "clearing the draw list restores the HUD-only frame byte for byte, and \
             the frame with items differs from it — so every pixel measured above \
             came from the item pass and none from a leftover buffer ({} bytes)",
            empty.len()
        ),
    );

    Ok(())
}
