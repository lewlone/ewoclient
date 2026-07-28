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
    OFFHAND_MENU_SLOT, QUICK_CRAFT_ONE, QUICK_CRAFT_SPLIT, SWAP_OFFHAND_BUTTON,
    slot_at, slot_contains, slot_position, ArmorPiece, Inventory, ItemProps, ItemSlot,
    ARMOR_MENU_START, HOTBAR_MENU_START, MENU_SLOTS,
};

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 85;
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
    check_names(&mut c, &baked, &jar)?;
    check_components(&mut c, &paths)?;
    check_enchantments(&mut c, &baked, &jar);
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
                == plain(276, 5)
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
        components: 0,
        damage: None,
        max_damage: None,
        enchanted: false,
    })
}

/// A stack carrying a patch, identified by its digest (M41). Two stacks with
/// the same `fingerprint` are the same components.
fn patched_stack(item_id: i32, count: i32, fingerprint: u64) -> Option<ItemSlot> {
    Some(ItemSlot {
        item_id,
        count,
        has_components: true,
        components: fingerprint,
        damage: None,
        max_damage: None,
        enchanted: false,
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
    slots4[9] = patched_stack(55, 3, 0xAAAA);
    inv4.set_content(1, &slots4, patched_stack(55, 5, 0xBBBB));
    let differing = inv4.click_pickup(9, 0, &props).unwrap();
    // …and the same two stacks with the *same* patch, which M35 could not tell
    // apart from the case above and therefore also swapped.
    let mut inv5 = Inventory::default();
    let mut slots5 = [None; MENU_SLOTS];
    slots5[9] = patched_stack(55, 3, 0xAAAA);
    inv5.set_content(1, &slots5, patched_stack(55, 5, 0xAAAA));
    let matching = inv5.click_pickup(9, 0, &props).unwrap();
    c.record(
        "s9.two_stacks_merge_when_their_components_match_and_swap_when_they_differ",
        differing.changed == vec![(9u16, patched_stack(55, 5, 0xBBBB))]
            && differing.carried == patched_stack(55, 3, 0xAAAA)
            && matching.changed == vec![(9u16, patched_stack(55, 8, 0xAAAA))]
            && matching.carried.is_none(),
        format!(
            "different patches swap ({:?}); identical ones merge to {:?}. M41 walks \
             the patch and digests it, so `isSameItemSameComponents` is exact — \
             M35 could only ask whether either side carried components at all, \
             which swapped both of these and left two identically-enchanted books \
             unable to stack",
            differing.changed[0].1,
            matching.changed[0].1
        ),
    );

    let helmet = |_id: i32| Some(props_for(1, Some(ArmorPiece::Head)));
    let dirt = |_id: i32| Some(props_for(64, None));

    // -- shift-click (QUICK_MOVE) ------------------------------------------
    //
    // The routing is not "the other half of the inventory": armour and the
    // off-hand are checked first, which is why shift-clicking a helmet equips
    // it rather than moving it.
    let mut qm = Inventory::default();
    let mut qs = [None; MENU_SLOTS];
    qs[36] = plain(55, 10); // dirt in the hotbar
    qm.set_content(1, &qs, None);
    let moved = qm.click_quick_move(36, &props).unwrap();
    c.record(
        "q1.the_hotbar_moves_up_into_the_main_grid",
        moved.changed.iter().any(|(s, v)| *s == 9 && *v == plain(55, 10))
            && moved.changed.iter().any(|(s, v)| *s == 36 && v.is_none()),
        format!(
            "slot 36 empties into slot 9: {:?}. `quickMoveStack` routes 36..45 to              9..36 and 9..36 to 36..45, so the two halves swap rather than              everything piling into one",
            moved.changed
        ),
    );

    let mut qm2 = Inventory::default();
    let mut qs2 = [None; MENU_SLOTS];
    qs2[9] = plain(55, 10);
    qm2.set_content(1, &qs2, None);
    let down = qm2.click_quick_move(9, &props).unwrap();
    c.record(
        "q2.the_main_grid_moves_down_into_the_hotbar",
        down.changed.iter().any(|(s, _)| *s == 36),
        format!("slot 9 lands in the hotbar: {:?}", down.changed),
    );

    // A helmet goes to the helmet slot first, and only if it is empty.
    let mut hm = Inventory::default();
    let mut hs = [None; MENU_SLOTS];
    hs[9] = plain(700, 1);
    hm.set_content(1, &hs, None);
    let equipped = hm.click_quick_move(9, &helmet).unwrap();
    hs[ARMOR_MENU_START] = plain(701, 1); // already wearing one
    let mut hm2 = Inventory::default();
    hm2.set_content(1, &hs, None);
    let not_equipped = hm2.click_quick_move(9, &helmet).unwrap();
    c.record(
        "q3.a_helmet_equips_before_it_travels_and_only_into_an_empty_slot",
        equipped.changed.iter().any(|(s, _)| *s == ARMOR_MENU_START as u16)
            && !not_equipped
                .changed
                .iter()
                .any(|(s, _)| *s == ARMOR_MENU_START as u16),
        format!(
            "with the head slot empty it equips ({:?}); with one already worn it              travels instead ({:?}). The armour check is guarded on the target              being empty, so shift-clicking a second helmet does not swap the              first out",
            equipped.changed,
            not_equipped.changed
        ),
    );

    // The merge pass fills an existing stack to its cap before using an empty.
    let mut mm = Inventory::default();
    let mut ms = [None; MENU_SLOTS];
    ms[36] = plain(55, 40);
    ms[9] = plain(55, 60);
    mm.set_content(1, &ms, None);
    let merged = mm.click_quick_move(36, &props).unwrap();
    let into_9 = merged.changed.iter().find(|(s, _)| *s == 9).and_then(|(_, v)| *v);
    c.record(
        "q4.quick_move_tops_up_an_existing_stack_before_taking_an_empty_slot",
        into_9 == plain(55, 64),
        format!(
            "slot 9 goes to {into_9:?} — filled to 64 first. `moveItemStackTo`              runs a merge pass over the whole range before its placement pass,              so shift-clicking never scatters a stack while a partial one waits"
        ),
    );

    // -- the keyboard actions (M40) ------------------------------------------
    //
    // 700 is this function's stand-in helmet; everything else stacks to 64.
    let mixed = |id: i32| {
        Some(if id == 700 {
            props_for(1, Some(ArmorPiece::Head))
        } else {
            props_for(64, None)
        })
    };

    // SWAP's button is an INVENTORY index while its slot is a MENU slot — two
    // numbers counted from different origins, which is the whole hazard.
    let mut swp = Inventory::default();
    let mut ss = [None; MENU_SLOTS];
    ss[36] = plain(700, 1); // a helmet at hotbar index 0
    swp.set_content(1, &ss, None);
    let swapped = swp
        .click_swap(ARMOR_MENU_START as i32, 0, &mixed)
        .map(|p| p.changed)
        .unwrap_or_default();
    // The same press with something the slot refuses. `Slot.mayPlace` gates a
    // swap exactly as it gates a placement, so a stack of dirt cannot be worn
    // on your head — which is also why the fixture above has to be a helmet.
    let mut plainswp = Inventory::default();
    let mut ps = [None; MENU_SLOTS];
    ps[36] = plain(1, 3);
    plainswp.set_content(1, &ps, None);
    let refused = plainswp
        .click_swap(ARMOR_MENU_START as i32, 0, &mixed)
        .map(|p| p.changed)
        .unwrap_or_default();
    c.record(
        "n1.swap_reads_its_button_as_an_inventory_index",
        swapped == vec![(ARMOR_MENU_START as u16, plain(700, 1)), (36u16, None)]
            && refused.is_empty(),
        format!(
            "pressing 1 over menu slot {ARMOR_MENU_START} equips: {swapped:?}. Button \
             0 is inventory index 0, which is **menu slot 36** — reading it as a menu \
             slot would have reached into the crafting grid instead. The same press \
             with a stack of dirt at that index changes {} slot(s), because \
             `mayPlace` gates a swap the same way it gates a placement",
            refused.len()
        ),
    );
    // Buttons 40 and 9, from an ordinary slot that actually holds something —
    // a swap between two empty slots is a no-op whatever the button, and would
    // have made this witness pass for the wrong reason.
    let mut rng = Inventory::default();
    let mut rs = [None; MENU_SLOTS];
    rs[9] = plain(1, 2);
    rng.set_content(1, &rs, None);
    let off = rng.click_swap(9, SWAP_OFFHAND_BUTTON, &mixed);
    let bad = rng.click_swap(9, 9, &mixed);
    c.record(
        "n2.the_swap_button_range_rejects_rather_than_clamps",
        off.is_some_and(|p| p.changed.contains(&(OFFHAND_MENU_SLOT as u16, plain(1, 2))))
            && bad.is_none(),
        format!(
            "button 40 sends the stack to menu slot {OFFHAND_MENU_SLOT}, and button 9 \
             predicts {} — the guard is `0..9 || == 40`, so 9 through 39 do nothing \
             at all. Clamping 9 to 8 would move a stack the player never named",
            bad.is_some()
        ),
    );

    // THROW: Q takes one, Ctrl+Q the stack, and neither runs while dragging.
    let mut thr = Inventory::default();
    let mut ts = [None; MENU_SLOTS];
    ts[9] = plain(1, 5);
    thr.set_content(1, &ts, None);
    let one = thr.click_throw(9, 0, &mixed).map(|p| p.changed);
    let all = thr.click_throw(9, 1, &mixed).map(|p| p.changed);
    c.record(
        "n3.throw_drops_one_or_the_whole_stack",
        one == Some(vec![(9u16, plain(1, 4))]) && all == Some(vec![(9u16, None)]),
        format!("button 0 leaves {one:?}, button 1 leaves {all:?}"),
    );
    let mut carrying = Inventory::default();
    carrying.set_content(1, &ts, plain(2, 1));
    c.record(
        "n4.throw_does_nothing_with_a_stack_on_the_cursor",
        carrying.click_throw(9, 0, &mixed).is_none(),
        "`doClick`'s guard is `getCarried().isEmpty()`, so Q over a slot while \
         dragging drops neither the slot's item nor the cursor's",
    );

    // PICKUP_ALL: the two-pass sweep, and the guard that stops an ordinary
    // second click from becoming one.
    let mut sweep = Inventory::default();
    let mut ws = [None; MENU_SLOTS];
    ws[10] = plain(1, 64); // full — pass 1 only
    ws[11] = plain(1, 5); // partial — pass 0
    ws[12] = plain(1, 7); // partial — pass 0
    sweep.set_content(1, &ws, plain(1, 1));
    let gathered = sweep.click_pickup_all(9, 0, &mixed);
    let carried_after = gathered.as_ref().and_then(|p| p.carried);
    let left = gathered.map(|p| p.changed).unwrap_or_default();
    c.record(
        "n5.pickup_all_takes_the_partial_stacks_before_the_full_one",
        carried_after == plain(1, 64)
            && left.contains(&(11u16, None))
            && left.contains(&(12u16, None))
            && left.contains(&(10u16, plain(1, 13))),
        format!(
            "the cursor ends at {carried_after:?} and the slots at {left:?}. \
             `pass != 0 || count != maxStackSize` skips a full stack on the first \
             pass, so the 5 and the 7 go first and only the remainder comes out of \
             the 64 — one pass would have emptied the full stack and left the \
             partials sitting there"
        ),
    );
    let mut occupied = Inventory::default();
    let mut os = [None; MENU_SLOTS];
    os[9] = plain(1, 3);
    os[11] = plain(1, 5);
    occupied.set_content(1, &os, plain(1, 1));
    c.record(
        "n6.pickup_all_needs_the_clicked_slot_to_be_empty",
        occupied.click_pickup_all(9, 0, &mixed).is_none(),
        "the guard is `!slot.hasItem() || !slot.mayPickup` — the second click of a \
         double click lands on the slot the first one emptied. Without it, any \
         second click on a full slot would hoover up the inventory",
    );

    // -- the drag (M40) ------------------------------------------------------
    //
    // One byte carries two fields, and reading it as one is the mistake that
    // turns a one-per-slot drag into an even spread.
    let masks = [
        Inventory::quick_craft_button(QUICK_CRAFT_SPLIT, 0),
        Inventory::quick_craft_button(QUICK_CRAFT_ONE, 1),
        Inventory::quick_craft_button(QUICK_CRAFT_ONE, 2),
    ];
    c.record(
        "d1.the_quick_craft_button_packs_a_type_and_a_header",
        masks == [0, 5, 6],
        format!(
            "{masks:?} — `type << 2 | header`, so type 1 header 1 is 5 and not 1. \
             `getQuickcraftType` reads `mask >> 2 & 3` and `getQuickcraftHeader` \
             `mask & 3`; sending a bare header makes every drag type 0"
        ),
    );

    // The even spread divides by the SLOT COUNT and floors.
    let mut dg = Inventory::default();
    let ds = [None; MENU_SLOTS];
    dg.set_content(1, &ds, plain(1, 3));
    let three_over_two = dg.click_quick_craft(&[9, 10], QUICK_CRAFT_SPLIT, &mixed);
    let counts: Vec<i32> = three_over_two
        .as_ref()
        .map(|p| p.changed.iter().filter_map(|(_, v)| v.map(|s| s.count)).collect())
        .unwrap_or_default();
    let leftover = three_over_two.as_ref().and_then(|p| p.carried);
    c.record(
        "d2.an_even_spread_floors_and_leaves_the_remainder_on_the_cursor",
        counts == vec![1, 1] && leftover == plain(1, 1),
        format!(
            "three items over two slots gives {counts:?} with {leftover:?} left. \
             `Mth.floor(count / slots)` is one each — not two and one, and not \
             an empty cursor"
        ),
    );
    let one_each = dg
        .click_quick_craft(&[9, 10], QUICK_CRAFT_ONE, &mixed)
        .and_then(|p| p.carried);
    c.record(
        "d3.the_one_per_slot_type_leaves_the_rest_behind",
        one_each == plain(1, 1),
        format!(
            "{one_each:?} — type 1 places exactly one per slot regardless of the \
             stack, so three over two slots keeps one on the cursor. Type 0 happens \
             to agree here, which is why d2 measures the counts and this the cursor"
        ),
    );

    // A one-slot drag is a click in disguise.
    c.record(
        "d4.a_single_slot_drag_collapses_into_a_pickup",
        Inventory::quick_craft_is_pickup(&[9], QUICK_CRAFT_ONE) == Some((9, 1))
            && Inventory::quick_craft_is_pickup(&[9, 10], QUICK_CRAFT_ONE).is_none()
            && dg.click_quick_craft(&[9], QUICK_CRAFT_SPLIT, &mixed).is_none(),
        "vanilla resets the quick-craft state and re-dispatches a one-slot drag as \
         `PICKUP` with `buttonNum = quickcraftType`. So the drag path must refuse \
         it — sending it as a drag would predict a spread the server never makes",
    );

    // A slot the drag cannot use never enters the set.
    let mut occ = Inventory::default();
    let mut oc = [None; MENU_SLOTS];
    oc[10] = plain(2, 1); // a different item
    occ.set_content(1, &oc, plain(1, 8));
    let accepted = occ.quick_craft_accepts(&[9, 10, 11], QUICK_CRAFT_SPLIT, &mixed);
    c.record(
        "d5.a_slot_holding_something_else_is_not_dragged_into",
        accepted == vec![9, 11],
        format!(
            "dragging over 9, 10 and 11 accepts {accepted:?} — slot 10 holds a \
             different item, so `canItemQuickReplace` refuses it and it never \
             reaches a packet. Filtering only at the end would still have sent an \
             add for it"
        ),
    );
    // …and a stack too small to feed them all stops claiming slots.
    let mut small = Inventory::default();
    small.set_content(1, &ds, plain(1, 2));
    let claimed = small.quick_craft_accepts(&[9, 10, 11], QUICK_CRAFT_SPLIT, &mixed);
    c.record(
        "d6.a_drag_claims_no_more_slots_than_the_stack_can_feed",
        claimed == vec![9, 10],
        format!(
            "two items dragged over three slots claim {claimed:?} — the guard is \
             `carried.getCount() > quickcraftSlots.size()` as each is added, so the \
             third is never taken and the spread stays one each"
        ),
    );

    // Nothing to move is not a change.
    let empty_src = qm.click_quick_move(20, &props);
    c.record(
        "q5.shift_clicking_an_empty_slot_predicts_nothing",
        empty_src.is_none(),
        "no prediction at all, so the caller sends no packet — an empty          changed-slot map would be a claim that the click did nothing, which          the server would answer with a resynchronisation",
    );

    // -- the slot rules -----------------------------------------------------
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
/// The display-name table (M40) — read from the jar's own `en_us.json`, so a
/// generated table cannot drift from the assets it describes.
fn check_names(
    c: &mut Checker,
    baked: &assets::BakedAssets,
    jar: &std::path::Path,
) -> Result<(), String> {
    let item_count = assets::jar_item_ids(jar)?.len();
    let raw = assets::jar_text(jar, "assets/minecraft/lang/en_us.json")
        .ok_or("en_us.json missing from the jar")?;
    let lang: std::collections::HashMap<String, String> =
        serde_json::from_str(&raw).map_err(|e| format!("parse lang: {e}"))?;

    // The names themselves. The tooltip is only as good as this lookup, and
    // it is the one part of M40 that reads a file nothing else in Rewo reads.
    let names = &baked.item_names;
    let sword = names.get("minecraft:diamond_sword").map(String::as_str);
    let dirt = names.get("minecraft:dirt").map(String::as_str);
    c.record(
        "t7.an_item_and_a_block_item_both_resolve_a_display_name",
        sword == Some("Diamond Sword") && dirt == Some("Dirt"),
        format!(
            "diamond_sword {sword:?}, dirt {dirt:?}. `Item.getDescriptionId` is              `item.minecraft.<id>`, but `BlockItem` overrides it to the block's              `block.minecraft.<id>` — `item.minecraft.dirt` does not exist, so              reading only the item spelling loses every block in the game"
        ),
    );
    // Every item must have one, or some slot in the inventory would hover
    // silently.
    let total = item_count;
    c.record(
        "t8.every_item_the_jar_ships_has_a_name",
        total > 0 && names.len() == total,
        format!(
            "{} names for {total} items",
            names.len()
        ),
    );
    // The seven items that carry both spellings. The preference is for the
    // block one, and it cannot be observed, because the two strings agree.
    let both = ["brewing_stand", "cauldron", "flower_pot", "nether_wart"];
    let disagree: Vec<&str> = both
        .into_iter()
        .filter(|n| {
            let l = |k: &str| lang.get(&format!("{k}.minecraft.{n}"));
            l("item") != l("block")
        })
        .collect();
    c.record(
        "t9.the_ambiguous_items_spell_their_name_the_same_either_way",
        disagree.is_empty(),
        format!(
            "of the items carrying both an `item.` and a `block.` key, {} disagree              ({disagree:?}) — so preferring the block spelling, which is what              `BlockItem.getDescriptionId` returns, is unobservable in 26.2.              Written down because a version where they diverge would pick silently",
            disagree.len()
        ),
    );

    Ok(())
}

/// The component walk (M41) — coverage, the values read out, and the digest
/// that makes `isSameItemSameComponents` exact.
fn check_components(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    use rewo_data::components::{DataComponentIds, DataComponentRegistry};
    use rewo_net::component_wire::{install_shapes, shape_for_id, CODECS};

    let registry = DataComponentRegistry::load(&paths.registries_json())?;
    let ids = DataComponentIds::load(&paths.registries_json())?;
    let installed = install_shapes(registry.ids());
    c.record(
        "m1.every_transcribed_codec_resolves_to_a_registry_id",
        installed == CODECS.len() && installed > 90,
        format!(
            "{installed} of {} table rows resolved against {} registered components. \
             A row whose name the registry does not know is dropped rather than \
             panicking, so this is what catches a component renamed by a version bump",
            CODECS.len(),
            registry.len()
        ),
    );
    // The fail-closed half: a component with no codec must stay unwalkable.
    // `minecraft:custom_data` is never network-synchronised, so it can never
    // appear on the wire and is deliberately absent from the table.
    let never_synced = registry.ids().get("minecraft:custom_data").copied();
    c.record(
        "m2.a_component_with_no_codec_is_not_walkable",
        never_synced.is_some_and(|id| shape_for_id(id).is_none())
            && shape_for_id(ids.damage).is_some(),
        format!(
            "custom_data (id {never_synced:?}) has no shape; damage (id {}) does. \
             The walk refuses what it cannot measure rather than skipping a \
             guessed number of bytes",
            ids.damage
        ),
    );

    // A synthetic patch, built exactly as the wire encodes one, walked through
    // the production decoder.
    let stack = |entries: &[(i32, Vec<u8>)], removed: &[i32]| -> Vec<u8> {
        let mut v = Vec::new();
        push_varint(&mut v, 1); // count
        push_varint(&mut v, 276); // item
        push_varint(&mut v, entries.len() as i32);
        push_varint(&mut v, removed.len() as i32);
        for (ty, value) in entries {
            push_varint(&mut v, *ty);
            v.extend_from_slice(value);
        }
        for ty in removed {
            push_varint(&mut v, *ty);
        }
        v
    };
    let varint = |n: i32| {
        let mut v = Vec::new();
        push_varint(&mut v, n);
        v
    };
    // An NBT string tag: type 8, then a modified-UTF8 length and bytes.
    let nbt_string = |s: &str| {
        let mut v = vec![8u8];
        v.extend_from_slice(&(s.len() as u16).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
        v
    };

    let read = |bytes: &[u8]| {
        let mut r = rewo_proto::reader::PacketReader::new(bytes);
        rewo_net::item_stack::read_optional(&mut r, ids).map(|s| (s, r.remaining()))
    };

    let damaged = stack(&[(ids.damage, varint(120))], &[]);
    let got = read(&damaged);
    let dmg = match &got {
        Ok((rewo_net::item_stack::WireSlot::Stack(s), rest)) => {
            (s.components.damage, s.aligned_stack(), *rest)
        }
        _ => (None, false, usize::MAX),
    };
    c.record(
        "m3.a_damage_patch_is_read_and_leaves_the_reader_aligned",
        dmg == (Some(120), true, 0),
        format!("damage {:?}, walked {}, {} byte(s) left over", dmg.0, dmg.1, dmg.2),
    );

    // Three entries, the middle one a component the decoder walks but does not
    // interpret — the whole point of the shape table is that the third is
    // still reached.
    let mixed = stack(
        &[
            (ids.unbreakable, Vec::new()), // `Unit` — **zero bytes**
            (
                registry.ids()["minecraft:enchantment_glint_override"],
                vec![1],
            ),
            (ids.damage, varint(7)),
        ],
        &[],
    );
    let after = match read(&mixed) {
        Ok((rewo_net::item_stack::WireSlot::Stack(s), rest)) => {
            (s.components.damage, s.components.unbreakable, rest)
        }
        _ => (None, false, usize::MAX),
    };
    c.record(
        "m4.the_walk_reaches_a_value_behind_two_others",
        after == (Some(7), true, 0),
        format!(
            "damage {:?} and unbreakable {} read from behind a zero-byte `Unit` and \
             an uninterpreted bool, with {} byte(s) left. Reading even one byte for \
             `unbreakable` would shift everything after it",
            after.0, after.1, after.2
        ),
    );

    // The digest: same entries, same answer; a different value, a different one.
    let a = read(&stack(&[(ids.damage, varint(5))], &[]));
    let b = read(&stack(&[(ids.damage, varint(5))], &[]));
    let d = read(&stack(&[(ids.damage, varint(6))], &[]));
    let fp = |r: &Result<(rewo_net::item_stack::WireSlot, usize), ()>| match r {
        Ok((rewo_net::item_stack::WireSlot::Stack(s), _)) => Some(s.components.fingerprint),
        _ => None,
    };
    c.record(
        "m5.the_component_digest_is_equal_for_equal_patches_and_not_otherwise",
        fp(&a).is_some() && fp(&a) == fp(&b) && fp(&a) != fp(&d),
        format!(
            "damage=5 twice gives {:?} and {:?}; damage=6 gives {:?}. This is what \
             makes `isSameItemSameComponents` exact — M35 answered it with \
             \"either side carries components\", which swapped every patched stack",
            fp(&a),
            fp(&b),
            fp(&d)
        ),
    );
    // A *removal* is not an absence, and must not digest like one.
    let removed = read(&stack(&[], &[ids.damage]));
    let empty = read(&stack(&[], &[]));
    c.record(
        "m6.a_removed_component_digests_differently_from_an_absent_one",
        fp(&removed).is_some() && fp(&removed) != fp(&empty),
        format!(
            "removing damage gives {:?} against an empty patch's {:?}. \
             `getOrDefault` answers a removal with the *type's* default rather \
             than the item's prototype, so the two are different stacks",
            fp(&removed),
            fp(&empty)
        ),
    );

    // A chat component is one NBT tag, which is why `custom_name` needs no
    // codec of its own.
    let named = read(&stack(&[(ids.custom_name, nbt_string("Old Faithful"))], &[]));
    let name = match &named {
        Ok((rewo_net::item_stack::WireSlot::Stack(s), rest)) => {
            (s.components.custom_name.clone(), *rest)
        }
        _ => (None, usize::MAX),
    };
    c.record(
        "m7.a_chat_component_reduces_to_one_nbt_tag",
        name == (Some("Old Faithful".to_string()), 0),
        format!(
            "custom_name {:?} with {} byte(s) left — `ComponentSerialization`'s \
             stream codec is `fromCodecWithRegistries`, which is one tag on the \
             wire, so the walk needs the NBT reader and not the chat codec",
            name.0, name.1
        ),
    );

    // The durability bar's arithmetic, which is not a proportion of what is
    // left but a count down from 13.
    let widths = [
        rewo_gpu::container::bar_width(0, 1561),
        rewo_gpu::container::bar_width(780, 1561),
        rewo_gpu::container::bar_width(1400, 1561),
        rewo_gpu::container::bar_width(1561, 1561),
    ];
    c.record(
        "m8.the_bar_counts_down_from_thirteen",
        widths == [13, 7, 1, 0],
        format!(
            "{widths:?} for damage 0, 780, 1400 and 1561 of 1561. \
             `round(13 - damage * 13 / max)` — computing it as \
             `13 * remaining / max` rounds the other way in the middle of the range"
        ),
    );
    let hues = [
        rewo_gpu::container::bar_color(0, 100),
        rewo_gpu::container::bar_color(50, 100),
        rewo_gpu::container::bar_color(100, 100),
    ];
    c.record(
        "m9.the_bar_runs_green_through_yellow_to_red",
        hues[0] == [0.0, 1.0, 0.0]
            && (hues[1][0] - 1.0).abs() < 1e-6
            && (hues[1][1] - 1.0).abs() < 1e-6
            && hues[2] == [1.0, 0.0, 0.0],
        format!(
            "full {:?}, half {:?}, empty {:?} — `hsvToRgb(health / 3, 1, 1)`, so a \
             third of the hue circle. Dividing by anything else lands the halfway \
             point off yellow",
            hues[0], hues[1], hues[2]
        ),
    );
    Ok(())
}

/// The enchantment registry and the tooltip lines it unlocks (M42).
fn check_enchantments(c: &mut Checker, baked: &assets::BakedAssets, jar: &std::path::Path) {
    use rewo_net::enchantment_parse::{parse_enchantment_registry, EnchantmentDef};

    let text = &baked.enchantment_text;
    // The two tags and the strings all come out of the client jar.
    c.record(
        "e1.the_client_jar_supplies_the_strings_and_the_two_tags",
        text.translate("enchantment.minecraft.sharpness") == Some("Sharpness")
            && text.level(5) == "V"
            && text.is_curse("minecraft:vanishing_curse")
            && !text.is_curse("minecraft:sharpness")
            && text.tooltip_rank("minecraft:vanishing_curse")
                < text.tooltip_rank("minecraft:sharpness"),
        format!(
            "sharpness={:?} level(5)={:?}; vanishing_curse is a curse={} at rank {:?} \
             against sharpness at {:?}. The tags live under `data/` in the client \
             jar — the vanilla datapack ships inside it, which is also where M19 \
             reads `ItemTags.SPEARS`",
            text.translate("enchantment.minecraft.sharpness"),
            text.level(5),
            text.is_curse("minecraft:vanishing_curse"),
            text.tooltip_rank("minecraft:vanishing_curse"),
            text.tooltip_rank("minecraft:sharpness"),
        ),
    );
    // A level past the ten `enchantment.level.N` keys 26.2 ships.
    c.record(
        "e2.a_level_without_a_numeral_falls_back_to_the_number",
        text.level(1) == "I" && text.level(10) == "X" && text.level(11) == "11",
        format!(
            "1={:?} 10={:?} 11={:?} — vanilla renders the raw key past ten; the \
             number is this milestone's one deliberate divergence, and it is \
             strictly more readable than `enchantment.level.11`",
            text.level(1),
            text.level(10),
            text.level(11)
        ),
    );

    // The registry parse, from bytes shaped exactly as `registry_data` sends
    // them: an identifier, then an optional NBT payload.
    let mut body: Vec<u8> = Vec::new();
    let mut entry = |name: &str, payload: Option<(&str, i32)>| {
        push_varint(&mut body, name.len() as i32);
        body.extend_from_slice(name.as_bytes());
        match payload {
            None => body.push(0),
            Some((key, max_level)) => {
                body.push(1);
                // A network NBT compound: {description:{translate:"…"},
                // max_level:<int>}. `max_level` is top-level because
                // `EnchantmentDefinition.CODEC` is a MapCodec and its fields
                // are inlined rather than nested under "definition".
                body.push(10); // TAG_Compound, unnamed
                body.push(10); // TAG_Compound "description"
                body.extend_from_slice(&(11u16).to_be_bytes());
                body.extend_from_slice(b"description");
                body.push(8); // TAG_String "translate"
                body.extend_from_slice(&(9u16).to_be_bytes());
                body.extend_from_slice(b"translate");
                body.extend_from_slice(&(key.len() as u16).to_be_bytes());
                body.extend_from_slice(key.as_bytes());
                body.push(0); // end description
                body.push(3); // TAG_Int "max_level"
                body.extend_from_slice(&(9u16).to_be_bytes());
                body.extend_from_slice(b"max_level");
                body.extend_from_slice(&max_level.to_be_bytes());
                body.push(0); // end root
            }
        }
    };
    entry("minecraft:sharpness", Some(("enchantment.minecraft.sharpness", 5)));
    entry("minecraft:mending", Some(("enchantment.minecraft.mending", 1)));
    entry("minecraft:vanishing_curse", Some(("enchantment.minecraft.vanishing_curse", 1)));
    entry("minecraft:nameless", None);

    let mut r = rewo_proto::reader::PacketReader::new(&body);
    let registry = parse_enchantment_registry(&mut r, 4);
    c.record(
        "e3.the_registry_parses_in_wire_order_with_its_max_levels",
        registry.len() == 4
            && registry[0].id == "minecraft:sharpness"
            && registry[0].max_level == 5
            && registry[1].max_level == 1
            && registry[3].description_key == "enchantment.minecraft.nameless",
        format!(
            "{} entries; [0]={} max {}, [1] max {}, [3] key {:?}. The index **is** \
             the protocol id, and an entry sent without its payload falls back to \
             the `makeDescriptionId` key rather than losing its slot",
            registry.len(),
            registry[0].id,
            registry[0].max_level,
            registry[1].max_level,
            registry[3].description_key
        ),
    );

    // The three rules, through the production line builder.
    let lines = crate::live_cmd::enchantment_lines(
        &[(0, 5), (1, 1), (2, 1)],
        &registry,
        text,
    );
    let names: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
    c.record(
        "e4.the_level_numeral_is_suppressed_only_when_the_maximum_is_also_one",
        names.contains(&"Sharpness V") && names.contains(&"Mending"),
        format!(
            "{names:?} — `getFullname` appends the numeral when \
             `level != 1 || maxLevel != 1`, so a level-1 Mending (max 1) has none \
             and a level-1 Sharpness (max 5) would. Suppressing on `level == 1` \
             alone loses the numeral from every single-level enchant applied"
        ),
    );
    let curse_first = names.first() == Some(&"Curse of Vanishing");
    let curse_red = lines
        .first()
        .is_some_and(|(_, col)| col[0] > 0.9 && col[1] < 0.5);
    c.record(
        "e5.a_curse_is_red_and_the_tooltip_order_tag_leads",
        curse_first && curse_red,
        format!(
            "first line {:?} coloured {:?}. The order is the \
             `minecraft:tooltip_order` tag, not the ids and not the stack's own \
             order — the curses sit at the top of that tag",
            names.first(),
            lines.first().map(|(_, c)| *c)
        ),
    );
    // An id the registry never synced yields no line at all.
    let unknown = crate::live_cmd::enchantment_lines(&[(99, 1)], &registry, text);
    c.record(
        "e6.an_unsynced_enchantment_id_yields_no_line",
        unknown.is_empty(),
        "an id past the registry's end is omitted rather than named — the server \
         sent an enchantment this session never synced, and inventing a name for \
         it would be worse than the omission",
    );
    let _ = jar;
}

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

    // -- the tooltip (M40) ---------------------------------------------------
    //
    // The layout is arithmetic, so it is checked as arithmetic first and as
    // pixels second. A one-line tooltip is the whole of what Rewo can show:
    // every further line vanilla adds comes from a component.
    let (one_w, one_h) = rewo_gpu::container::tooltip_size(&[40]);
    let (two_w, two_h) = rewo_gpu::container::tooltip_size(&[40, 55]);
    c.record(
        "t1.a_single_line_tooltip_is_two_pixels_shorter_than_its_line",
        (one_w, one_h) == (40, 8) && (two_w, two_h) == (55, 20),
        format!(
            "one line -> {one_w}x{one_h}, two -> {two_w}x{two_h}.              `tempHeight = lines.size() == 1 ? -2 : 0` then += 10 per line, so one              line measures 8 and two measure 20, not 10 and 20. The width is the              widest line, not their sum"
        ),
    );
    // The positioner's horizontal recovery is a *flip*, not a clamp.
    let clear = rewo_gpu::container::tooltip_position(400, 300, 100, 100, 50, 8);
    let tight = rewo_gpu::container::tooltip_position(400, 300, 380, 100, 50, 8);
    let cramped = rewo_gpu::container::tooltip_position(60, 300, 50, 100, 50, 8);
    c.record(
        "t2.a_tooltip_near_the_right_edge_flips_to_the_other_side",
        clear == (112, 88) && tight == (318, 88) && cramped == (4, 88),
        format!(
            "clear {clear:?}, tight {tight:?}, cramped {cramped:?}. Away from the              edge it sits at the cursor +(12, -12); against it the x becomes              `max(x - 24 - w, 4)` — and the `x` there is the **already-offset**              392, not the raw cursor's 380, so the answer is 318 and not 306.              Reading it as the cursor puts the tooltip twelve pixels too far left;              a clamp instead of a flip would have given 350"
        ),
    );
    // The vertical recovery, both ways: it fires only once `y + h + 3` really
    // is past the bottom, which at `h = 8` means a cursor below 301 — not
    // merely one near the edge.
    let near = rewo_gpu::container::tooltip_position(400, 300, 100, 295, 50, 8);
    let low = rewo_gpu::container::tooltip_position(400, 300, 100, 305, 50, 8);
    c.record(
        "t3.the_vertical_recovery_is_a_clamp_and_uses_only_the_padding",
        near == (112, 283) && low == (112, 289),
        format!(
            "at cursor y 295 the box sits at {near:?} untouched; at 305 it is              {low:?} — `y + h + 3 > screenHeight` pushes it to              `screenHeight - (h + 3)` = 289, so the box's *border art* is allowed              off the bottom. Using the full 12 px inset here would lift it three              pixels too high, and clamping unconditionally would move the first case"
        ),
    );

    // And the pixels. Hovering a slot with an item must draw a box that is not
    // there when the slot is empty.
    let tip_at = |wr: &mut WorldRenderer, gpu: &mut Gpu, off: &mut Offscreen, t| -> Result<Vec<u8>, String> {
        wr.set_container(true, rewo_world::inventory::slot_position(9));
        wr.set_container_tooltip(t);
        let f = shot(gpu, off, wr, &[])?;
        wr.set_container_tooltip(None);
        Ok(f)
    };
    let no_tip = tip_at(wr, gpu, off, None)?;
    let with_tip = tip_at(wr, gpu, off, Some(((60, 40), (40, 8))))?;
    let tip_box = changed_bounds(&no_tip, &with_tip);
    // The sprite is blitted `TOOLTIP_INSET` outside the text on every side.
    let inset = rewo_gpu::container::TOOLTIP_INSET;
    // **Screen space, not panel space.** The positioner works from the whole
    // screen's GUI size, because a tooltip flips against the screen's edge —
    // so its result is not offset by the panel's origin the way a slot's is,
    // and the text pass places the lines the same way. Adding `left`/`top`
    // here put the box a panel's width from its own text, and this witness
    // agreed with it until it was rewritten to bracket the *text*.
    let want = (
        ((60 - inset) as f32 * scale) as u32,
        ((40 - inset) as f32 * scale) as u32,
        ((60 + 40 + inset) as f32 * scale) as u32,
        ((40 + 8 + inset) as f32 * scale) as u32,
    );
    // The property that matters: the box has to contain where the text lands.
    let text_origin = ((60.0 * scale) as u32, (40.0 * scale) as u32);
    let fits = tip_box.is_some_and(|(x0, y0, x1, y1)| {
        x0 >= want.0
            && y0 >= want.1
            && x1 <= want.2
            && y1 <= want.3
            && x1 > x0
            && y1 > y0
            // …and it brackets the text, which is the half a box drawn in the
            // wrong coordinate space fails.
            && x0 <= text_origin.0
            && y0 <= text_origin.1
            && x1 >= text_origin.0
            && y1 >= text_origin.1
    });
    c.record(
        "t4.the_tooltip_box_covers_the_text_plus_its_inset_and_no_more",
        fits,
        format!(
            "the box changes {tip_box:?}, inside the padded rect {want:?} —              `PADDING` 3 + `MARGIN` 9 outside a 40x8 text block. A box drawn at the              text's own rect, or at the full 100x100 sprite size, falls outside this"
        ),
    );
    c.record(
        "t5.no_tooltip_is_byte_identical_to_the_plain_screen",
        no_tip == hovering && with_tip != hovering,
        "passing `None` restores the hovering frame byte for byte, and passing a          box changes it — so every pixel t4 measured came from the tooltip and          none from a stale buffer",
    );
    // Two boxes at different places must not land in the same pixels: a pass
    // that ignored the origin would pass t4 by accident.
    let moved = tip_at(wr, gpu, off, Some(((100, 90), (40, 8))))?;
    c.record(
        "t6.the_tooltip_follows_its_origin",
        changed_bounds(&no_tip, &moved) != tip_box,
        format!(
            "at (60,40) the box is {tip_box:?}; at (100,90) it is {:?}",
            changed_bounds(&no_tip, &moved)
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
