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
use rewo_world::inventory::{
    slot_at, slot_contains, slot_position, ArmorPiece, Inventory, ItemProps, ItemSlot,
    HOTBAR_MENU_START, MENU_SLOTS,
};

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 44;
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
    check_screen(&mut c);
    check_click_packet(&mut c, &paths)?;
    check_preview(&mut c);
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
                    has_components: false,
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


// -- 4. the screen: layout, hover, and the click arithmetic (M35) -------------

/// A stack with no components, which is every case the arithmetic below cares
/// about — the component-bearing case is its own witness.
fn plain(item_id: i32, count: i32) -> Option<ItemSlot> {
    Some(ItemSlot {
        item_id,
        count,
        has_components: false,
    })
}

/// The two facts a click needs about an item, resolved here rather than from
/// the generated table, so the arithmetic witnesses grade the arithmetic and
/// not the table (which has its own tests).
fn props_for(max_stack: i32, equips: Option<ArmorPiece>) -> ItemProps {
    ItemProps { max_stack, equips }
}

fn check_screen(c: &mut Checker) {
    // -- the layout ---------------------------------------------------------
    //
    // Independent transcription of `InventoryMenu`'s constructor. Not read
    // from `slot_position`: these are the numbers the decompile prints, and
    // the point is that the two agree.
    let mut expected: Vec<(usize, (i32, i32))> = vec![
        (0, (154, 28)),   // addResultSlot(owner, 154, 28)
        (1, (98, 18)),    // addCraftingGridSlots(98, 18): x + y*2
        (2, (116, 18)),
        (3, (98, 36)),
        (4, (116, 36)),
        (5, (8, 8)),      // ArmorSlot(..., 8, 8 + i * 18), head first
        (6, (8, 26)),
        (7, (8, 44)),
        (8, (8, 62)),
        (45, (77, 62)),   // Slot(inventory, 40, 77, 62)
    ];
    // addStandardInventorySlots(inventory, 8, 84): three rows of nine, then
    // the hotbar at top + 58.
    for row in 0..3 {
        for col in 0..9 {
            expected.push((9 + row * 9 + col, (8 + col as i32 * 18, 84 + row as i32 * 18)));
        }
    }
    for col in 0..9 {
        expected.push((36 + col, (8 + col as i32 * 18, 142)));
    }
    let wrong: Vec<_> = expected
        .iter()
        .filter(|(slot, pos)| slot_position(*slot) != Some(*pos))
        .collect();
    c.record(
        "s1.every_menu_slot_sits_where_InventoryMenu_puts_it",
        wrong.is_empty() && expected.len() == MENU_SLOTS,
        format!(
            "{} of {MENU_SLOTS} slots matched an independent transcription of the \
             menu constructor{}",
            expected.len() - wrong.len(),
            if wrong.is_empty() {
                String::new()
            } else {
                format!("; first mismatch {:?}", wrong[0])
            }
        ),
    );

    c.record(
        "s2.the_hotbar_row_is_58_below_the_main_grid_not_54",
        slot_position(36).map(|p| p.1) == Some(142)
            && slot_position(27).map(|p| p.1) == Some(120),
        format!(
            "main row 3 at y={:?}, hotbar at y={:?} — a 22 px gap. Three rows of 18 \
             would put the hotbar at 138; vanilla's `topToHotbar` is a named 58, and \
             guessing 3*18 leaves every hotbar icon four pixels high",
            slot_position(27).map(|p| p.1),
            slot_position(36).map(|p| p.1)
        ),
    );

    // -- the hover box ------------------------------------------------------
    let (sx, sy) = slot_position(9).unwrap();
    let inside = slot_contains(9, sx as f64 - 1.0, sy as f64 - 1.0)
        && slot_contains(9, (sx + 16) as f64, (sy + 16) as f64);
    let outside = !slot_contains(9, sx as f64 - 1.1, sy as f64)
        && !slot_contains(9, (sx + 17) as f64, sy as f64);
    c.record(
        "s3.the_hover_box_is_18_wide_not_16",
        inside && outside,
        format!(
            "slot 9 at {:?} answers for {sx}-1 .. {sx}+16 inclusive. \
             `isHovering` is `x >= left - 1 && x < left + w + 1` with w = 16, so the \
             box bleeds a pixel on every side and neighbouring slots tile without a \
             gap — testing the 16 px icon rect instead leaves a dead cross between \
             every pair",
            (sx, sy)
        ),
    );

    // Adjacent slots are 18 apart and both boxes are 18 wide, so the bleed
    // makes them abut exactly rather than overlap.
    let gaps: Vec<usize> = (9..17)
        .filter(|&s| {
            let (x, y) = slot_position(s).unwrap();
            // The pixel column just past this slot's bleed belongs to the next.
            slot_at((x + 17) as f64, y as f64) != Some(s + 1)
        })
        .collect();
    c.record(
        "s4.neighbouring_slots_tile_without_a_dead_column",
        gaps.is_empty(),
        format!("{} of 8 boundaries in the first main row hand straight over", 8 - gaps.len()),
    );

    // -- the click arithmetic ----------------------------------------------
    //
    // Every case drives the production `click_pickup`; the expectations are
    // read off `AbstractContainerMenu.doClick`'s PICKUP branch by hand.
    let props = |_id: i32| Some(props_for(64, None));
    let mut inv = Inventory::default();
    let mut slots = [None; MENU_SLOTS];
    slots[36] = plain(55, 42); // dirt
    inv.set_content(1, &slots, None);

    let take_all = inv.click_pickup(36, 0, &props).unwrap();
    let take_half = inv.click_pickup(36, 1, &props).unwrap();
    c.record(
        "s5.a_primary_click_takes_the_stack_and_a_secondary_takes_half_rounded_up",
        take_all.carried == plain(55, 42)
            && take_all.changed == vec![(36u16, None)]
            && take_half.carried == plain(55, 21)
            && take_half.changed == vec![(36u16, plain(55, 21))],
        format!(
            "42 → carried {:?} / {:?}. The half is `(count + 1) / 2`, so an odd stack \
             rounds **up** onto the cursor — 43 would give 22, not 21",
            take_all.carried.map(|s| s.count),
            take_half.carried.map(|s| s.count)
        ),
    );

    // Merging into a partial stack of the same item, capped by the item's own
    // max stack size.
    let mut inv2 = Inventory::default();
    let mut slots2 = [None; MENU_SLOTS];
    slots2[9] = plain(55, 60);
    inv2.set_content(1, &slots2, plain(55, 20));
    let merge = inv2.click_pickup(9, 0, &props).unwrap();
    c.record(
        "s6.a_merge_fills_the_slot_to_its_cap_and_leaves_the_rest_on_the_cursor",
        merge.changed == vec![(9u16, plain(55, 64))] && merge.carried == plain(55, 16),
        format!(
            "60 + 20 → slot {:?}, cursor {:?}. `safeInsert` transfers \
             `min(amount, count, cap - occupied)` = min(20, 20, 4) = 4",
            merge.changed.first().and_then(|c| c.1).map(|s| s.count),
            merge.carried.map(|s| s.count)
        ),
    );

    // A secondary click places exactly one.
    let one = inv2.click_pickup(9, 1, &props).unwrap();
    c.record(
        "s7.a_secondary_click_places_exactly_one",
        one.changed == vec![(9u16, plain(55, 61))] && one.carried == plain(55, 19),
        format!(
            "slot {:?}, cursor {:?} — the secondary amount is a literal 1, not half \
             of the carried stack",
            one.changed.first().and_then(|c| c.1).map(|s| s.count),
            one.carried.map(|s| s.count)
        ),
    );

    // Different items swap.
    let mut inv3 = Inventory::default();
    let mut slots3 = [None; MENU_SLOTS];
    slots3[9] = plain(55, 3);
    inv3.set_content(1, &slots3, plain(1, 5));
    let swap = inv3.click_pickup(9, 0, &props).unwrap();
    c.record(
        "s8.two_different_items_swap",
        swap.changed == vec![(9u16, plain(1, 5))] && swap.carried == plain(55, 3),
        format!("slot takes {:?}, cursor takes {:?}", swap.changed[0].1, swap.carried),
    );

    // Components make two stacks of the same item unlike, so they swap rather
    // than merging. Rewo cannot compare components, and this is the safe half
    // of that ignorance.
    let mut inv4 = Inventory::default();
    let mut slots4 = [None; MENU_SLOTS];
    slots4[9] = Some(ItemSlot {
        item_id: 55,
        count: 3,
        has_components: true,
    });
    inv4.set_content(1, &slots4, plain(55, 5));
    let patched = inv4.click_pickup(9, 0, &props).unwrap();
    c.record(
        "s9.a_component_bearing_stack_swaps_rather_than_merging",
        patched.changed == vec![(9u16, plain(55, 5))]
            && patched.carried.map(|s| s.has_components) == Some(true),
        format!(
            "same item id, but the slot's stack carries components: {:?} → the two \
             swap. Rewo decodes *whether* a patch was present, never what it held, \
             so treating a patched stack as unique is the direction that cannot \
             destroy anything — the server corrects a missed merge, but a wrong \
             merge would have fused two different tools",
            patched.changed[0].1
        ),
    );

    // -- the slot rules -----------------------------------------------------
    let helmet = |_id: i32| Some(props_for(1, Some(ArmorPiece::Head)));
    let dirt = |_id: i32| Some(props_for(64, None));
    let mut inv5 = Inventory::default();
    inv5.set_content(1, &[None; MENU_SLOTS], plain(700, 1));
    let into_head = inv5.click_pickup(5, 0, &helmet).unwrap();
    let into_chest = inv5.click_pickup(6, 0, &helmet).unwrap();
    c.record(
        "s10.a_helmet_goes_in_the_helmet_slot_and_nowhere_else",
        into_head.changed == vec![(5u16, plain(700, 1))] && into_chest.changed.is_empty(),
        format!(
            "head slot takes it ({:?}), chest slot refuses ({} change(s)) — \
             `ArmorSlot.mayPlace` is `isEquippableInSlot`, which compares the item's \
             own `minecraft:equippable` slot",
            into_head.changed.first().and_then(|c| c.1),
            into_chest.changed.len()
        ),
    );

    let mut inv6 = Inventory::default();
    inv6.set_content(1, &[None; MENU_SLOTS], plain(55, 5));
    let dirt_in_armor = inv6.click_pickup(5, 0, &dirt).unwrap();
    c.record(
        "s11.an_item_with_no_equippable_component_is_refused_by_every_armour_slot",
        dirt_in_armor.changed.is_empty() && dirt_in_armor.carried == plain(55, 5),
        "`isEquippableInSlot` treats a missing component as main-hand-only, so \
         absence refuses rather than defaulting to allowed — a table that returned \
         a default slot would let dirt onto your head",
    );

    let dirt_in_offhand = inv6.click_pickup(45, 0, &dirt).unwrap();
    c.record(
        "s12.the_offhand_accepts_anything",
        dirt_in_offhand.changed == vec![(45u16, plain(55, 5))],
        "slot 45 is a plain `Slot` in `InventoryMenu`, not an `ArmorSlot` — its \
         shield icon is decoration, and vanilla really will hold a stack of dirt there",
    );

    let mut inv7 = Inventory::default();
    let mut slots7 = [None; MENU_SLOTS];
    slots7[0] = plain(55, 4);
    inv7.set_content(1, &slots7, plain(55, 1));
    let into_result = inv7.click_pickup(0, 0, &dirt).unwrap();
    c.record(
        "s13.the_result_slot_refuses_a_placement_but_gives_up_its_contents",
        into_result.changed == vec![(0u16, None)] && into_result.carried == plain(55, 5),
        format!(
            "cursor {:?} → the crafting output cannot be placed into, but the same \
             item on the cursor collects from it. `ResultSlot.mayPlace` is a literal \
             `false`, and the else-arm is the take",
            into_result.carried.map(|s| s.count)
        ),
    );

    // An item this build cannot resolve declines the whole click.
    let unknown = |_id: i32| None;
    let declined = inv.click_pickup(36, 0, &unknown);
    c.record(
        "s14.an_unresolvable_item_declines_the_click_entirely",
        declined.is_none(),
        "no prediction at all, rather than one against a guessed stack cap — the \
         same rule the item render path uses for the definitions M22 suppresses. A \
         guessed cap would be sent as fact and resynced",
    );

    // A button that is not 0 or 1 is not a PICKUP.
    c.record(
        "s15.only_the_two_pickup_buttons_are_accepted",
        inv.click_pickup(36, 2, &props).is_none() && inv.click_pickup(36, -1, &props).is_none(),
        "quick-move, swap, throw and quick-craft each have their own arm in \
         `doClick` and none is reproduced, so the caller must not be able to send \
         one with an unpredicted changed-slot map",
    );
}

/// The serverbound click packet's bytes, built by the production writer.
fn check_click_packet(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    let packets = Packets::load(&paths.packets_json())?;
    let ids = rewo_net::ids::Ids::resolve(&packets)?;
    c.record(
        "k1.the_click_packet_resolves_by_name",
        ids.sb_play_container_click.is_some(),
        format!(
            "serverbound `container_click` is id {:?}",
            ids.sb_play_container_click
        ),
    );

    let bytes = rewo_net::hashed_stack_bytes(plain(276, 5));
    c.record(
        "k2.a_hashed_stack_is_optional_then_raw_id_then_count_then_two_empty_maps",
        bytes == vec![1, 0x94, 0x02, 5, 0, 0],
        format!(
            "{bytes:?} — present(1), item 276 as a **raw** var-int (`holderRegistry`, \
             not `holder`'s id+1 scheme), count 5, then `HashedPatchMap`'s *two* \
             collections. One zero instead of two would misalign every following slot"
        ),
    );

    let empty = rewo_net::hashed_stack_bytes(None);
    c.record(
        "k3.an_empty_stack_is_a_single_false",
        empty == vec![0],
        "`HashedStack.EMPTY` is the absent case of `ByteBufCodecs.optional`, so it is \
         one byte and nothing follows — not a present stack with count zero",
    );
    Ok(())
}


// -- 5. the player preview (M36) ----------------------------------------------

fn check_preview(c: &mut Checker) {
    use rewo_gpu::container::{gui_origin, preview_angles, preview_rect, preview_view_proj, PREVIEW};

    // An independent transcription of the call `InventoryScreen` makes:
    // `extractEntityInInventoryFollowsMouse(g, xo + 26, yo + 8, xo + 75, yo + 78, 30, 0.0625F, ...)`.
    c.record(
        "v1.the_preview_window_is_the_one_InventoryScreen_asks_for",
        PREVIEW == (26, 8, 75, 78),
        format!(
            "GUI {PREVIEW:?} — 49x70, which is exactly the region `inventory.png` \
             paints black. The texture has that window so the model has something \
             to stand against; drawing the model anywhere else leaves a black hole \
             and a player floating over the slots"
        ),
    );

    let (sw, sh) = (1280.0f32, 720.0f32);
    let (rx, ry, rw, rh) = preview_rect(sw, sh);
    let (left, top, scale) = gui_origin(sw, sh);
    c.record(
        "v2.the_window_scales_with_the_panel",
        rx == left + 26.0 * scale
            && ry == top + 8.0 * scale
            && rw == 49.0 * scale
            && rh == 70.0 * scale,
        format!(
            "at GUI scale {scale} the window is {rw}x{rh} px at ({rx}, {ry}) — the \
             same origin the panel is drawn from, so the model cannot drift out of \
             its hole when the window is resized"
        ),
    );

    // `xAngle = atan((centreX - mouseX) / 40)`, `* 20` at every use.
    let centre = (
        left as f64 + (PREVIEW.0 + PREVIEW.2) as f64 / 2.0 * scale as f64,
        top as f64 + (PREVIEW.1 + PREVIEW.3) as f64 / 2.0 * scale as f64,
    );
    let at_centre = preview_angles(centre, sw, sh);
    let left_of = preview_angles((centre.0 - 60.0, centre.1), sw, sh);
    let right_of = preview_angles((centre.0 + 60.0, centre.1), sw, sh);
    let far = preview_angles((0.0, 0.0), sw, sh);
    c.record(
        "v3.the_model_turns_toward_the_cursor_and_the_turn_is_bounded",
        at_centre.0.abs() < 1e-5
            && at_centre.1.abs() < 1e-5
            && (left_of.0 + right_of.0).abs() < 1e-5
            && left_of.0 > 0.0
            && far.0.abs() < 20.0 * std::f32::consts::FRAC_PI_2,
        format!(
            "centre {at_centre:?}, 60 px either side {:.3} / {:.3}, a corner \
             {:.3}. `atan` is what bounds it: the model can turn at most \
             20 * pi/2 = 31.4 degrees however far the cursor goes, so it never \
             spins to face away",
            left_of.0, right_of.0, far.0
        ),
    );

    // The model's feet and head, projected through the production matrix and
    // compared against the placement computed here from the decompile:
    //   T(w/2, h/2, 0) . S(s, s, -s) . T(0, bbH/2 + 0.0625, 0) . Rz(pi) . ...
    // Rz(pi) negates y, so a point at model height `m` lands at
    //   h/2 + (bbH/2 + 0.0625 - m) * s
    // in window-local pixels, y down.
    const BB: f32 = 1.8;
    let vp = glam::Mat4::from_cols_array_2d(&preview_view_proj(sw, sh, BB, 0.0));
    let project = |m: f32| -> f32 {
        let clip = vp * glam::Vec4::new(0.0, m, 0.0, 1.0);
        // Clip -> NDC -> window-local pixels, y down (the entity pass's
        // viewport is flipped, so NDC +1 is the top).
        let ndc_y = clip.y / clip.w;
        (1.0 - ndc_y) / 2.0 * sh - ry
    };
    let s = scale * 30.0;
    let want = |m: f32| rh / 2.0 + (BB / 2.0 + 0.0625 - m) * s;
    let (feet, head) = (project(0.0), project(BB));
    c.record(
        "v4.the_model_stands_where_the_transform_says",
        (feet - want(0.0)).abs() < 0.01 && (head - want(BB)).abs() < 0.01,
        format!(
            "feet at {feet:.1} px down the window (want {:.1}), head at {head:.1} \
             (want {:.1}), in a window {rh} tall. `offsetY` = 0.0625 lifts it a \
             sixteenth of a block so the feet clear the bottom edge",
            want(0.0),
            want(BB)
        ),
    );

    // The camera half-turn. `GuiEntityRenderer` ends with
    // `orientation.rotateY(PI)`, and `bodyRot = 180 + xAngle` already points
    // the model away from an unturned camera — so dropping it shows the back.
    // Here that is observable as a mirrored x: the same model point lands on
    // the opposite side of the window.
    let no_turn = {
        let (_, _, gs) = gui_origin(sw, sh);
        let s = gs * 30.0;
        let mv = glam::Mat4::from_translation(glam::Vec3::new(rw / 2.0, rh / 2.0, 0.0))
            * glam::Mat4::from_scale(glam::Vec3::new(s, s, -s))
            * glam::Mat4::from_translation(glam::Vec3::new(0.0, BB / 2.0 + 0.0625, 0.0))
            * glam::Mat4::from_rotation_z(std::f32::consts::PI);
        mv
    };
    let arm = glam::Vec4::new(0.35, 1.4, 0.0, 1.0);
    let turned = vp * arm;
    let plain = {
        let p = no_turn * arm;
        // Through the same clip mapping the production matrix ends with.
        (p.x + rx) / sw * 2.0 - 1.0
    };
    let turned_x = turned.x / turned.w;
    // Mirrored about the **window's** centre, not about clip zero — the window
    // sits left of the screen's middle, so both land negative and a sign test
    // would pass on a matrix with no half turn at all.
    let centre_clip = 2.0 * (rx + rw / 2.0) / sw - 1.0;
    c.record(
        "v5.the_camera_half_turn_is_applied",
        ((turned_x - centre_clip) + (plain - centre_clip)).abs() < 1e-4
            && (turned_x - plain).abs() > 1e-3,
        format!(
            "the point 0.35 blocks to the model's right lands at clip x {turned_x:.4} \
             with the half turn and {plain:.4} without, either side of the window's \
             centre at {centre_clip:.4} — mirrored. Without the turn the model is \
             drawn back-to-front, which is what the first build did"
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


    // -- the screen (M35) ---------------------------------------------------
    let Some(sprites) = crate::live_cmd::container_sprites(baked) else {
        return Err("container sprites missing from the jar".into());
    };
    let panel_w = sprites.background.w;
    let panel_rgba: Vec<u8> = sprites.background.rgba.to_vec();
    wr.init_container(gpu, &sprites)?;

    // No screen, then the screen closed-over-the-world, then hovering slot 9.
    wr.set_gui_items(gpu, &[])?;
    wr.set_container(false, None);
    let world_only = shot(gpu, off, wr, &[])?;
    wr.set_container(true, None);
    let open = shot(gpu, off, wr, &[])?;
    wr.set_container(true, rewo_world::inventory::slot_position(9));
    let hovering = shot(gpu, off, wr, &[])?;
    if let Some(d) = &args.out_dir {
        let _ = off.save_png(gpu, &d.join("inventoryshot-screen.png"));
    }

    let (left, top, scale) = rewo_gpu::container::gui_origin(W as f32, H as f32);
    let at = |img: &[u8], gx: i32, gy: i32| -> [u8; 3] {
        // The centre of a GUI pixel, so the sample cannot land on a seam.
        let x = (left + (gx as f32 + 0.5) * scale) as u32;
        let y = (top + (gy as f32 + 0.5) * scale) as u32;
        let i = ((y.min(H - 1) * W + x.min(W - 1)) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let src = |gx: i32, gy: i32| -> [u8; 3] {
        let i = ((gy as u32 * panel_w + gx as u32) * 4) as usize;
        [panel_rgba[i], panel_rgba[i + 1], panel_rgba[i + 2]]
    };

    // Points inside flat regions of the panel, away from every bevel.
    let probes = [(20, 80), (60, 110), (120, 30), (90, 150), (170, 160), (4, 100)];
    let wrong: Vec<_> = probes
        .iter()
        .filter(|&&(x, y)| at(&open, x, y) != src(x, y))
        .collect();
    c.record(
        "gs1.the_panel_reproduces_its_texture_byte_for_byte",
        wrong.is_empty(),
        format!(
            "{}/{} probes match `inventory.png` exactly{} — the sRGB round trip \
             (SRGB image view decodes, SRGB attachment re-encodes) has to cancel, \
             and a single linearisation either way would shift every one of them",
            probes.len() - wrong.len(),
            probes.len(),
            wrong
                .first()
                .map(|&&(x, y)| format!(
                    "; ({x},{y}) rendered {:?} against source {:?}",
                    at(&open, x, y),
                    src(x, y)
                ))
                .unwrap_or_default()
        ),
    );

    // Outside the panel, the world is still visible but darker.
    let corner = ((left as u32 / 2).max(2), (top as u32 / 2).max(2));
    let i = ((corner.1 * W + corner.0) * 4) as usize;
    let before: u32 = world_only[i..i + 3].iter().map(|&v| v as u32).sum();
    let after: u32 = open[i..i + 3].iter().map(|&v| v as u32).sum();
    c.record(
        "gs2.the_backdrop_dims_the_world_without_hiding_it",
        after < before && after > 0,
        format!(
            "the world outside the panel goes from {before} to {after} (summed \
             channels) — `renderTransparentBackground` is a ~75% near-black wash, \
             not an opaque fill, so the world stays legible behind the screen"
        ),
    );

    // The highlight lands on the hovered slot, four pixels out on every side.
    let changed_box = changed_bounds(&hovering, &open);
    let want = rewo_world::inventory::slot_position(9).unwrap();
    let expect = (
        (left + (want.0 - 4) as f32 * scale) as u32,
        (top + (want.1 - 4) as f32 * scale) as u32,
    );
    let ok = changed_box.is_some_and(|(x0, y0, x1, y1)| {
        let size = (24.0 * scale) as u32;
        x0 >= expect.0
            && y0 >= expect.1
            && x1 <= expect.0 + size
            && y1 <= expect.1 + size
            && x1 > x0
    });
    c.record(
        "gs3.the_hover_highlight_sits_on_the_slot_and_only_there",
        ok,
        format!(
            "hovering slot 9 changes {changed_box:?}, inside the 24x24 box at \
             {expect:?}. The sprite is drawn at `slot - 4` and 24 px wide — a four \
             pixel bleed around the 16 px icon, so anchoring it at the slot itself \
             would sit down and right of where vanilla puts it"
        ),
    );

    c.record(
        "gs4.no_hover_means_no_highlight",
        changed_bounds(&open, &open).is_none() && changed_box.is_some(),
        "with nothing under the cursor the frame is exactly the panel — the \
         highlight is not drawn somewhere harmless, it is not drawn",
    );

    // And the closed screen is not merely a dimmer world: the panel is gone.
    c.record(
        "gs5.closing_the_screen_removes_it_entirely",
        at(&world_only, 90, 150) != at(&open, 90, 150),
        format!(
            "the panel's centre is {:?} closed against {:?} open",
            at(&world_only, 90, 150),
            at(&open, 90, 150)
        ),
    );

    // Put the screen away again, so the closing witness below still compares
    // like with like — it asserts byte-identity against a frame captured
    // before the screen existed.
    wr.set_container(false, None);

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
