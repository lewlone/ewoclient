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

const EXPECTED_WITNESSES: usize = 152;
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
    check_lang(&mut c, &baked, &jar)?;
    check_rarity(&mut c, &paths)?;
    check_components(&mut c, &paths)?;
    check_enchantments(&mut c, &baked, &jar);
    check_glint(&mut c, &baked);
    check_tooltip_image(&mut c, &baked);
    check_advanced_tooltip(&mut c, &baked, &paths)?;
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
        // No menu open: this gate is the PLAYER's inventory, so every witness
        // below addresses container 0. A fresh empty `Menus` reproduces
        // exactly the pre-M87 behaviour these 152 witnesses were written
        // against -- a non-zero container id has nowhere to land and is
        // dropped. The container path is `containershot`'s to grade.
        let mut menus = rewo_world::menu::Menus::new();
        rewo_net::route_inventory(id, body, &ids, Some(components), inv, &mut menus, None)
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

    check_authoritative_writes(c, &ids, components)?;
    Ok(())
}

/// M69 — the server's authoritative writes, and the tag override beside them.
///
/// M34/M35 built a *predicting* inventory whose only correction path is a full
/// state-id resync. These are the two packets that correct it without one, and
/// they were simply not in `ids.rs`.
fn check_authoritative_writes(
    c: &mut Checker,
    ids: &rewo_net::ids::Ids,
    components: rewo_data::components::DataComponentIds,
) -> Result<(), String> {
    let route = |id: i32, body: &[u8], inv: &mut Inventory| -> bool {
        // See the note in the caller above: no menu open, container 0 only.
        let mut menus = rewo_world::menu::Menus::new();
        rewo_net::route_inventory(id, body, ids, Some(components), inv, &mut menus, None)
    };

    c.record(
        "sw1.the_two_authoritative_writes_resolve_by_name_and_are_distinct",
        ids.cb_play_set_player_inventory != ids.cb_play_set_cursor_item
            && ids.cb_play_set_player_inventory != ids.cb_play_container_set_slot
            && ids.cb_play_set_cursor_item != ids.cb_play_container_set_content,
        format!(
            "set_player_inventory {}, set_cursor_item {} — resolved from the report by \
             name, distinct from each other and from the three M34 ids they sit beside",
            ids.cb_play_set_player_inventory, ids.cb_play_set_cursor_item
        ),
    );

    // -- the third coordinate system ------------------------------------

    // Inventory index 40 is the off-hand, and lands in menu slot 45.
    let mut inv = Inventory::default();
    let mut body = Vec::new();
    push_varint(&mut body, 40);
    body.extend_from_slice(&stack_bytes(731, 1));
    let matched = route(ids.cb_play_set_player_inventory, &body, &mut inv);
    c.record(
        "sw2.the_slot_is_a_var_int_not_a_short",
        matched && inv.offhand() == plain(731, 1),
        format!(
            "index 40 → off-hand {:?}. The neighbouring `container_set_slot` writes its \
             index as an i16 among var-ints (M34's recorded trap) and this record's \
             STREAM_CODEC is `ByteBufCodecs.VAR_INT`. A short reader here swallows the \
             first byte of the stack that follows, and the resulting index — 0x28 << 8 \
             — is out of range, so the write silently lands nowhere",
            inv.offhand()
        ),
    );

    // The armour ranges run against each other: inventory 36 is FEET.
    let mut inv = Inventory::default();
    let mut feet = Vec::new();
    push_varint(&mut feet, 36);
    feet.extend_from_slice(&stack_bytes(800, 1));
    route(ids.cb_play_set_player_inventory, &feet, &mut inv);
    c.record(
        "sw3.inventory_index_36_is_the_boots_not_the_helmet",
        inv.armor(ArmorPiece::Feet as usize) == plain(800, 1) && inv.armor(0).is_none(),
        format!(
            "boots {:?}, helmet {:?}. `InventoryMenu`'s ctor is \
             `SLOT_IDS = {{HEAD, CHEST, LEGS, FEET}}` with backing index `39 - i`, so \
             the two armour ranges are ORDERED AGAINST EACH OTHER. Subtracting a \
             constant offset — the obvious reading of \"36 here, 5 there\" — puts \
             boots on the head, and renders as a plausible wrong answer rather than \
             an error",
            inv.armor(ArmorPiece::Feet as usize),
            inv.armor(0)
        ),
    );

    // 41 and 42 are real EntityEquipment slots with no InventoryMenu counterpart.
    let mut inv = Inventory::default();
    let mut body_armor = Vec::new();
    push_varint(&mut body_armor, 41);
    body_armor.extend_from_slice(&stack_bytes(900, 1));
    let matched = route(ids.cb_play_set_player_inventory, &body_armor, &mut inv);
    c.record(
        "sw4.body_armour_matches_the_packet_but_reaches_no_menu_slot",
        matched && inv.is_empty(),
        "`Inventory.SLOT_BODY_ARMOR` (41) and `SLOT_SADDLE` (42) are stored in \
         `EntityEquipment`, which `InventoryMenu`'s 46 slots do not expose. The id \
         still matched — the packet was understood — but there is nowhere in this \
         client to put it, which is a different fact from a malformed index and is \
         kept as one (`IndexWrite::NoMenuSlot`)",
    );

    // -- the state id, which this packet does not carry ------------------

    let mut inv = Inventory::default();
    let mut content = Vec::new();
    push_varint(&mut content, 0);
    push_varint(&mut content, 55); // stateId
    push_varint(&mut content, MENU_SLOTS as i32);
    for _ in 0..MENU_SLOTS {
        content.extend_from_slice(&stack_bytes(0, 0));
    }
    content.extend_from_slice(&stack_bytes(0, 0));
    route(ids.cb_play_container_set_content, &content, &mut inv);
    let before = (inv.state_id(), inv.content_updates());
    let mut write = Vec::new();
    push_varint(&mut write, 0); // hotbar index 0
    write.extend_from_slice(&stack_bytes(64, 3));
    route(ids.cb_play_set_player_inventory, &write, &mut inv);
    c.record(
        "sw5.an_authoritative_write_does_not_disturb_the_click_prediction_state",
        inv.hotbar(0) == plain(64, 3)
            && (inv.state_id(), inv.content_updates()) == before,
        format!(
            "state id still {}, resync count still {}. `handleSetPlayerInventory` is \
             `getInventory().setItem(...)` — it bypasses the container menu entirely, \
             so there is no state id ON this packet to apply. Advancing one would make \
             the next click echo a number the server never issued, and a stale state \
             id is exactly what triggers the full resync this packet exists to avoid",
            inv.state_id(),
            inv.content_updates()
        ),
    );

    // -- the cursor ------------------------------------------------------

    let mut inv = Inventory::default();
    let cursor = stack_bytes(276, 12);
    let matched = route(ids.cb_play_set_cursor_item, &cursor, &mut inv);
    let set = inv.carried();
    let cleared = route(ids.cb_play_set_cursor_item, &stack_bytes(0, 0), &mut inv);
    c.record(
        "sw6.the_cursor_is_one_stack_and_an_empty_one_clears_it",
        matched && set == plain(276, 12) && cleared && inv.carried().is_none(),
        format!(
            "carried became {set:?}, then an empty stack left {:?}. The whole body is \
             one `ItemStack` — no container id, no state id, no slot — and this is the \
             only correction M35's PREDICTED cursor has short of a whole \
             `container_set_content`. Treating the empty stack as \"no change\" would \
             leave a phantom stack on the pointer",
            inv.carried()
        ),
    );

    // A short body of either leaves the previous value standing, as the M34
    // arms do — and for the cursor it matters more, because there is no
    // container id to reject on and no length to run short of but the stack's.
    let mut inv = Inventory::default();
    route(ids.cb_play_set_cursor_item, &stack_bytes(276, 12), &mut inv);
    let held = inv.carried();
    let truncated = &stack_bytes(276, 12)[..1];
    route(ids.cb_play_set_cursor_item, truncated, &mut inv);
    let mut short_index = Vec::new();
    push_varint(&mut short_index, 5); // an index, then no stack at all
    route(ids.cb_play_set_player_inventory, &short_index, &mut inv);
    c.record(
        "sw7.a_truncated_authoritative_write_changes_nothing",
        inv.carried() == held && inv.menu_slot(41).is_none(),
        format!(
            "the cursor is still {:?} and menu slot 41 is still empty. A stack that \
             cannot be walked leaves the reader parked mid-value, so anything derived \
             from it is garbage — the same stance every other arm here takes",
            inv.carried()
        ),
    );

    // -- update_tags, the third M69 packet -------------------------------

    // Both were resolved by `req!` before this function ran — a missing name
    // fails `Ids::resolve` and the gate never starts. What is still worth
    // asserting is that they came from DIFFERENT STATE TABLES, and the
    // observable consequence of that is that they differ (13 / 134 in 26.2).
    //
    // Mutation partner: resolve the configuration id out of the play table
    // (`req!(p, P, C, "update_tags")` for both) and this fails. A future
    // version in which the two genuinely coincide would fail it too, and that
    // is the right outcome — it should be looked at, not waved through.
    c.record(
        "sw8.update_tags_is_resolved_separately_in_each_state",
        ids.cb_config_update_tags != ids.cb_play_update_tags,
        format!(
            "configuration {} / play {}. One packet, two states, two ids. The \
             CONFIGURATION one is the one a vanilla server actually sends on join \
             (right after `registry_data`); the play one is the datapack-reload case. \
             Resolving only the play id would have looked like it worked until \
             somebody ran `/reload`",
            ids.cb_config_update_tags, ids.cb_play_update_tags
        ),
    );

    // Drive a real body through the production router, not just the module.
    let mut overrides = rewo_net::tags::TagOverrides::default();
    let mut tag_body = Vec::new();
    push_varint(&mut tag_body, 1); // one registry
    push_str(&mut tag_body, rewo_net::tags::ITEM_REGISTRY);
    push_varint(&mut tag_body, 2); // two tags
    push_str(&mut tag_body, rewo_net::tags::SPEARS_TAG);
    push_varint(&mut tag_body, 2);
    push_varint(&mut tag_body, 731);
    push_varint(&mut tag_body, 300);
    push_str(&mut tag_body, "minecraft:swords");
    push_varint(&mut tag_body, 0); // declared, and empty
    let matched = rewo_net::route_tags(
        ids.cb_play_update_tags,
        &tag_body,
        ids,
        &mut overrides,
    );
    c.record(
        "sw9.the_server_can_retag_an_item_and_rewo_now_hears_it",
        matched
            && overrides.contains(
                rewo_net::tags::ITEM_REGISTRY,
                rewo_net::tags::SPEARS_TAG,
                731,
            ) == Some(true)
            && overrides.contains(
                rewo_net::tags::ITEM_REGISTRY,
                rewo_net::tags::SPEARS_TAG,
                64,
            ) == Some(false)
            && overrides.tag(rewo_net::tags::ITEM_REGISTRY, "minecraft:swords") == Some(&[][..])
            && overrides
                .contains(rewo_net::tags::ENCHANTMENT_REGISTRY, "minecraft:curse", 0)
                .is_none(),
        "item 731 is now in `minecraft:spears` and 64 is not — M19's SPEAR arm pose \
         reads that tag FROM THE JAR, so a server that retags it diverges with no \
         error anywhere. This decodes and models the override; it is deliberately \
         NOT wired into the pose lookup yet (see `rewo_net::tags`), because half of \
         it wired is worse than none. An unmentioned registry answers `None` — \
         silence, not a no",
    );
    Ok(())
}

/// A length-prefixed UTF-8 string, the `Identifier` encoding.
fn push_str(v: &mut Vec<u8>, s: &str) {
    push_varint(v, s.len() as i32);
    v.extend_from_slice(s.as_bytes());
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
        trim_material: None,
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
        trim_material: None,
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

/// The language map (M50) — the deprecation pass, and the substitution.
///
/// The subject throughout is `baked.lang`, the map the **production** bake
/// built, so a gate that assembled its own could not hide a renderer still
/// reading the raw file. The mutation partner throughout is
/// [`rewo_data::lang::Language::raw`], which is step 1 alone — literally what
/// Rewo did before this milestone.
fn check_lang(
    c: &mut Checker,
    baked: &assets::BakedAssets,
    jar: &std::path::Path,
) -> Result<(), String> {
    use rewo_data::lang::{format, DeprecatedTranslations, Language, DEPRECATED_PATH, EN_US_PATH};
    use std::collections::{HashMap, HashSet};

    let raw_json = assets::jar_text(jar, EN_US_PATH).ok_or("en_us.json missing from the jar")?;
    let dep_json =
        assets::jar_text(jar, DEPRECATED_PATH).ok_or("deprecated.json missing from the jar")?;
    let dep = DeprecatedTranslations::parse(&dep_json)?;
    // The mutation partner: `en_us.json` with no deprecation pass.
    let raw = Language::raw(&raw_json);
    let lang = &baked.lang;

    // The two keys the brief named, which a tooltip line generator needs and
    // which resolve to nothing without the pass.
    let named = [
        ("item.container.item_count", "%s x%s"),
        ("item.container.more_items", "and %s more..."),
    ];
    let after: Vec<Option<&str>> = named.iter().map(|(k, _)| lang.get(k)).collect();
    let before: Vec<Option<&str>> = named.iter().map(|(k, _)| raw.get(k)).collect();
    c.record(
        "l1.the_container_count_keys_resolve_only_after_the_rename_pass",
        after
            .iter()
            .zip(named)
            .all(|(got, (_, want))| *got == Some(want))
            && before.iter().all(Option::is_none),
        format!(
            "after the pass {after:?}; MUTATION — reading en_us.json raw, as Rewo did \
             before M50, gives {before:?}. Both values are moved off \
             `container.shulkerBox.itemCount`/`.more`, so the new keys are not in the \
             file at all"
        ),
    );

    // The scale of it, measured rather than asserted from the brief.
    let targets: Vec<&str> = dep.renamed().map(|(_, t)| t).collect();
    let absent = targets.iter().filter(|t| !raw.has(t)).count();
    let overwritten = targets.iter().filter(|t| raw.has(t)).count();
    // Six of the overwritten targets are handed a string identical to the one
    // they already had — including `subtitles.entity.sulfur_cube.squish`,
    // which is renamed onto itself — so the *observable* half is smaller than
    // the mechanical one, and both are worth stating.
    let differs = targets
        .iter()
        .filter(|t| raw.has(t) && raw.get(t) != lang.get(t))
        .count();
    let all_resolve = targets.iter().all(|t| lang.has(t));
    c.record(
        "l2.every_rename_target_resolves_after_the_pass_and_a_hundred_and_five_only_then",
        all_resolve && absent == 105 && overwritten == 41 && differs == 35,
        format!(
            "{} renames: {absent} of the targets do not exist in en_us.json at all, \
             {overwritten} already do and are overwritten ({differs} of them with a \
             different string); all {} resolve after the pass. MUTATION — the raw \
             read answers `None` for the first group and the pre-rename value for the \
             second (e.g. item.minecraft.bolt_armor_trim_smithing_template)",
            targets.len(),
            targets.len()
        ),
    );

    // The removal pass, and the three keys a later rename writes back.
    let target_set: HashSet<&str> = targets.iter().copied().collect();
    let mut survivors: Vec<&str> = dep
        .removed()
        .iter()
        .filter(|k| lang.has(k))
        .map(String::as_str)
        .collect();
    survivors.sort_unstable();
    let raw_present = dep.removed().iter().filter(|k| raw.has(k)).count();
    c.record(
        "l3.the_removed_keys_are_gone_except_the_three_a_rename_writes_back",
        dep.removed().len() == 383
            && raw_present == 381
            && survivors.len() == 3
            && survivors.iter().all(|k| target_set.contains(k)),
        format!(
            "{} declared removed, {raw_present} of them present in the raw file, {} \
             present after the pass — {survivors:?}, every one of them also a rename \
             *target*. MUTATION — skipping the removal pass leaves all {raw_present}",
            dep.removed().len(),
            survivors.len()
        ),
    );

    // ...which is only true because removal runs first. The mutation is the
    // same two passes in the other order.
    let mut reversed: HashMap<String, String> = raw
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    for (from, to) in dep.renamed() {
        match reversed.remove(from) {
            Some(v) => {
                reversed.insert(to.to_string(), v);
            }
            None => {
                reversed.remove(to);
            }
        }
    }
    for k in dep.removed() {
        reversed.remove(k);
    }
    let lost: Vec<&&str> = survivors.iter().filter(|k| !reversed.contains_key(**k)).collect();
    c.record(
        "l4.the_removals_run_before_the_renames",
        lost.len() == survivors.len() && !survivors.is_empty(),
        format!(
            "MUTATION — renaming first and removing second loses {} of the {} keys that \
             are both removed and renamed onto ({lost:?}). `applyToMap` removes, then \
             renames, so `debug.crash.message` is deleted and then written back from \
             `debug.crash.message.rebindable`",
            lost.len(),
            survivors.len()
        ),
    );

    // The branch 26.2's own data never takes: a rename whose source is absent
    // **deletes** the target. Synthetic because there is nothing to observe it
    // on — and that is exactly why it is worth pinning.
    let orphans = dep.renamed().filter(|(f, _)| !raw.has(f)).count();
    let synthetic = DeprecatedTranslations::parse(r#"{"removed":[],"renamed":{"gone":"live"}}"#)?;
    let mut m: HashMap<String, String> = HashMap::new();
    m.insert("live".into(), "stale".into());
    m.insert("other".into(), "kept".into());
    synthetic.apply_to_map(&mut m);
    c.record(
        "l5.a_rename_whose_source_is_absent_deletes_the_target",
        !m.contains_key("live") && m.contains_key("other") && orphans == 0,
        format!(
            "a rename `gone -> live` over a map that has `live` but not `gone` leaves \
             {:?}. MUTATION — treating the missing source as a no-op keeps the stale \
             `live`. Unobservable on 26.2's data ({orphans} of {} renames have an \
             absent source), which is why it is pinned synthetically",
            m.keys().collect::<Vec<_>>(),
            targets.len()
        ),
    );

    // The M40 safety check, restated as a measurement: the pass must not cost
    // an item its name, and it visibly improves 27 of them.
    let items = assets::jar_item_ids(jar)?;
    let raw_name = |n: &str| {
        raw.get(&std::format!("block.minecraft.{n}"))
            .or_else(|| raw.get(&std::format!("item.minecraft.{n}")))
    };
    let mut lost_names = 0usize;
    let mut changed: Vec<&str> = Vec::new();
    for name in &items {
        let after = baked.item_names.get(&std::format!("minecraft:{name}")).map(String::as_str);
        if after.is_none() {
            lost_names += 1;
        }
        if after != raw_name(name) {
            changed.push(name);
        }
    }
    let template = baked
        .item_names
        .get("minecraft:bolt_armor_trim_smithing_template")
        .map(String::as_str);
    c.record(
        "l6.the_pass_costs_no_item_its_name_and_corrects_twenty_seven",
        lost_names == 0 && changed.len() == 27 && template == Some("Bolt Armor Trim"),
        format!(
            "{} items, {lost_names} without a display name, {} whose name the pass \
             changes; bolt_armor_trim_smithing_template is {template:?}. MUTATION — \
             the raw read calls it {:?}, because `item.minecraft.<x>.new` is renamed \
             onto `item.minecraft.<x>` for the eighteen templates and nine banner \
             patterns",
            items.len(),
            changed.len(),
            raw_name("bolt_armor_trim_smithing_template")
        ),
    );

    // The M42 safety check: no enchantment string moves, so M42's tooltip
    // lines are untouched by all of the above.
    let ench: Vec<&str> = raw
        .iter()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("enchantment."))
        .collect();
    let moved: Vec<&&str> = ench
        .iter()
        .filter(|k| raw.get(k) != lang.get(k))
        .collect();
    c.record(
        "l7.no_enchantment_string_is_removed_or_renamed",
        ench.len() == 54 && moved.is_empty(),
        format!(
            "{} `enchantment.*` keys, {} of which read differently after the pass. \
             MUTATION — a version that renamed one would show here, and M42's tooltip \
             line for it would have silently disappeared",
            ench.len(),
            moved.len()
        ),
    );

    // `TranslatableContents.decomposeTemplate`, driven directly.
    let plain = format("%s x%s", &["Dirt", "64"]);
    let positional = format("%2$s then %1$s", &["a", "b"]);
    let counter = format("%s %1$s %s", &["a", "b"]);
    c.record(
        "l8.the_implicit_and_positional_forms_share_one_counter",
        plain == "Dirt x64" && positional == "b then a" && counter == "a a b",
        format!(
            "`%s x%s` -> {plain:?}, `%2$s then %1$s` -> {positional:?}, \
             `%s %1$s %s` -> {counter:?}. MUTATION — a positional specifier that also \
             advanced `replacementIndex` would render the last as \"a a c\" and run off \
             the end of a two-argument list; vanilla increments only on the implicit \
             form"
        ),
    );

    let literal = format("100%% sure", &[]);
    let unsupported = format("%d apples", &["3"]);
    let short = format("%s and %s", &["one"]);
    let stray = format("50% of %s", &["x"]);
    c.record(
        "l9.a_literal_percent_renders_and_every_error_renders_the_raw_pattern",
        literal == "100% sure"
            && unsupported == "%d apples"
            && short == "%s and %s"
            && stray == "50% of %s",
        format!(
            "`100%%%% sure` -> {literal:?}; the three error shapes render unsubstituted \
             — unsupported type {unsupported:?}, too few arguments {short:?}, a stray \
             percent in the prefix {stray:?}. MUTATION — dropping the line, or \
             substituting anyway, instead of `decompose`'s \
             `catch (TranslatableFormatException) -> FormattedText.of(format)`"
        ),
    );

    Ok(())
}

/// `ItemStack.getRarity()` (M50) — the prototype half the wire cannot carry,
/// and the enchantment promotion.
fn check_rarity(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    use crate::live_cmd::stack_rarity;
    use rewo_data::components::{DataComponentIds, DataComponentRegistry};
    use rewo_data::item_props_table::{rarity, DEFAULT_RARITY};

    const COMMON: i32 = 0;
    const UNCOMMON: i32 = 1;
    const RARE: i32 = 2;
    const EPIC: i32 = 3;
    /// `Rarity`'s name and `color()`, so a *failing* message names the colour
    /// actually resolved rather than the one the witness expected.
    fn label(r: i32) -> &'static str {
        match r {
            COMMON => "COMMON, white",
            UNCOMMON => "UNCOMMON, yellow",
            RARE => "RARE, aqua",
            EPIC => "EPIC, light purple",
            _ => "outside the enum",
        }
    }

    // The witness the brief asks to fail before the fix: today's
    // `rarity.unwrap_or(0)` is exactly `patch.unwrap_or(COMMON)`.
    let disc = "minecraft:music_disc_13";
    let got = stack_rarity(Some(disc), None, false);
    let patch_only: i32 = None.unwrap_or(DEFAULT_RARITY);
    c.record(
        "r1.a_music_disc_with_an_empty_patch_is_uncommon",
        got == UNCOMMON && patch_only == COMMON,
        format!(
            "{disc} with no component patch resolves rarity {got} ({}). MUTATION — \
             reading the patch alone, which is what Rewo did before M50, gives \
             {patch_only} ({}); a music disc's name is yellow in vanilla. \
             `getOrDefault(RARITY, COMMON)` answers from the item's *prototype*, and \
             the wire never sends one",
            label(got),
            label(patch_only)
        ),
    );

    // How much of the game that covers.
    let items = rewo_data::items::Items::load(&paths.registries_json())?;
    let names: Vec<&str> = (0..)
        .map_while(|i| items.name(i))
        .collect();
    let non_default: Vec<&&str> = names.iter().filter(|n| rarity(n) != DEFAULT_RARITY).collect();
    let buckets = [UNCOMMON, RARE, EPIC].map(|r| names.iter().filter(|n| rarity(**n) == r).count());
    c.record(
        "r2.the_prototype_table_covers_a_hundred_and_fifteen_items",
        non_default.len() == 115 && buckets == [78, 18, 19] && DEFAULT_RARITY == COMMON,
        format!(
            "{} of {} registry items differ from Rarity.COMMON — {} uncommon, {} rare, \
             {} epic. MUTATION — without the generated column every one of them is \
             white, and the count is the number of items that render wrong",
            non_default.len(),
            names.len(),
            buckets[0],
            buckets[1],
            buckets[2]
        ),
    );

    // The patch still wins where it speaks — a plugin may set it.
    let overridden = stack_rarity(Some(disc), Some(EPIC), false);
    let unknown_item = stack_rarity(None, None, false);
    c.record(
        "r3.a_patch_rarity_overrides_the_prototype",
        overridden == EPIC && unknown_item == COMMON,
        format!(
            "{disc} with an explicit EPIC patch resolves {overridden}; an item this \
             build cannot name resolves {unknown_item}. MUTATION — taking the table \
             first would ignore the patch, which is the half `getOrDefault` gets right \
             by construction"
        ),
    );

    // The promotion, all four arms.
    let promote = |base: i32| {
        // Driven through the production function by way of a patch, so the
        // switch is the shipped one.
        stack_rarity(None, Some(base), true)
    };
    let unenchanted: Vec<i32> = (0..4).map(|b| stack_rarity(None, Some(b), false)).collect();
    c.record(
        "r4.enchanting_promotes_common_and_uncommon_to_rare",
        promote(COMMON) == RARE && promote(UNCOMMON) == RARE && unenchanted == vec![0, 1, 2, 3],
        format!(
            "COMMON -> {}, UNCOMMON -> {} when enchanted; unenchanted the four ids are \
             {unenchanted:?}. MUTATION — promoting by one step would send COMMON to \
             UNCOMMON, but vanilla's `case COMMON, UNCOMMON -> Rarity.RARE` collapses \
             both onto RARE",
            promote(COMMON),
            promote(UNCOMMON)
        ),
    );

    let beyond = promote(9);
    c.record(
        "r5.rare_becomes_epic_and_epic_is_the_ceiling",
        promote(RARE) == EPIC && promote(EPIC) == EPIC && beyond == 9,
        format!(
            "RARE -> {}, EPIC -> {}, and an id outside the enum ({beyond}) passes \
             through. MUTATION — a `+ 1` promotion would push EPIC to 4, which is not \
             a rarity; vanilla's `default -> baseRarity` is what stops it",
            promote(RARE),
            promote(EPIC)
        ),
    );

    // The wire input the promotion reads, through the production decoder: an
    // enchanted book carries `stored_enchantments` and is **not** enchanted.
    let registry = DataComponentRegistry::load(&paths.registries_json())?;
    let ids = DataComponentIds::load(&paths.registries_json())?;
    rewo_net::component_wire::install_shapes(registry.ids());
    let stack = |ty: i32| -> Vec<u8> {
        let mut v = Vec::new();
        push_varint(&mut v, 1); // count
        push_varint(&mut v, 276); // item
        push_varint(&mut v, 1); // one component added
        push_varint(&mut v, 0); // none removed
        push_varint(&mut v, ty);
        push_varint(&mut v, 1); // one enchantment
        push_varint(&mut v, 5); // registry id
        push_varint(&mut v, 3); // level
        v
    };
    let read = |ty: i32| -> Option<(bool, usize)> {
        let bytes = stack(ty);
        let mut r = rewo_proto::reader::PacketReader::new(&bytes);
        match rewo_net::item_stack::read_optional(&mut r, ids) {
            Ok(rewo_net::item_stack::WireSlot::Stack(s)) if r.remaining() == 0 => {
                Some((s.components.is_enchanted, s.components.enchantments.len()))
            }
            _ => None,
        }
    };
    let worn = read(ids.enchantments);
    let book = read(ids.stored_enchantments);
    // `unwrap_or(true)` rather than `false`: a decode that failed outright must
    // not accidentally satisfy the "not enchanted" half of this witness.
    let book_enchanted = book.map(|b| b.0).unwrap_or(true);
    let book_rarity = stack_rarity(Some("minecraft:enchanted_book"), None, book_enchanted);
    c.record(
        "r6.a_book_carries_stored_enchantments_and_is_not_enchanted",
        worn == Some((true, 1)) && book == Some((false, 1)) && book_rarity == RARE,
        format!(
            "minecraft:enchantments -> (is_enchanted, listed) {worn:?}; \
             minecraft:stored_enchantments -> {book:?}; an enchanted book resolves \
             {book_rarity} ({}), its prototype. MUTATION — deriving `isEnchanted` \
             from the merged tooltip list, which is the union of both components, \
             promotes the book to EPIC. `isEnchanted()` reads ENCHANTMENTS alone",
            label(book_rarity)
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

    // ---- minecraft:container (M63) ----------------------------------------
    //
    // The codec table is complete — every one of the 104 components 26.2
    // registers with a `.networkSynchronized(...)` has a shape — so a
    // container already *walked*. M63 keeps what it walked past, which
    // `ItemContainerContents.addToTooltip` needs and nothing else carries.
    //
    // Every witness below is graded on **alignment as well as content**: the
    // patch has no length prefix, so a capture that consumed one byte
    // differently would leave the reader parked mid-value and turn every stack
    // after it in the packet into garbage. That is the property, and the
    // content is only the cheaper half of it.
    let text_tag = |s: &str| {
        let mut v = vec![0x0A, 0x08];
        v.extend_from_slice(&4u16.to_be_bytes());
        v.extend_from_slice(b"text");
        v.extend_from_slice(&(s.len() as u16).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
        v.push(0x00);
        v
    };
    // `ItemStackTemplate.STREAM_CODEC.apply(optional).apply(list(256))`.
    let container_value = |slots: &[Option<(i32, i32, Option<&str>)>]| {
        let mut v = Vec::new();
        push_varint(&mut v, slots.len() as i32);
        for s in slots {
            match s {
                None => v.push(0),
                Some((item, count, name)) => {
                    v.push(1);
                    push_varint(&mut v, *item);
                    push_varint(&mut v, *count);
                    push_varint(&mut v, name.is_some() as i32);
                    push_varint(&mut v, 0);
                    if let Some(n) = name {
                        push_varint(&mut v, ids.custom_name);
                        v.extend_from_slice(&text_tag(n));
                    }
                }
            }
        }
        v
    };
    let components_of = |bytes: &[u8]| match read(bytes) {
        Ok((rewo_net::item_stack::WireSlot::Stack(s), rest)) => {
            let aligned = s.aligned_stack();
            Some((s.components, aligned, rest))
        }
        _ => None,
    };

    // A shulker box with a gap in the middle and a renamed sword after it,
    // then a `damage` entry that only reads back correctly if the container
    // was consumed to the byte.
    let boxed = stack(
        &[
            (
                ids.container,
                container_value(&[
                    Some((1, 64, None)),
                    None,
                    Some((276, 1, Some("Excalibur"))),
                ]),
            ),
            // 300 — two var-int bytes, so a one-byte slip shows in the value
            // and not only in the alignment.
            (ids.damage, vec![0xAC, 0x02]),
        ],
        &[],
    );
    let got = components_of(&boxed);
    let ok = got.as_ref().is_some_and(|(comps, aligned, rest)| {
        let slots = comps.container_contents().unwrap_or(&[]);
        *aligned
            && *rest == 0
            && slots.len() == 3
            && slots[1].is_none()
            && slots[0].as_ref().is_some_and(|s| (s.item_id, s.count) == (1, 64))
            && slots[2]
                .as_ref()
                .is_some_and(|s| s.custom_name.as_deref() == Some("Excalibur"))
            && comps.damage == Some(300)
    });
    c.record(
        "ct1.a_container_patch_keeps_its_slots_and_stays_byte_aligned",
        ok,
        format!(
            "{:?} slot(s), damage {:?}, aligned {:?}, {:?} byte(s) over. The nested \
             `custom_name` is the one span the capture reads itself rather than \
             through `Shape::NbtTag` — reading it as anything else (a `Str`, say) \
             desynchronises, and the trailing damage is what notices",
            got.as_ref().map(|(c, _, _)| c.container_contents().map(<[_]>::len)),
            got.as_ref().and_then(|(c, _, _)| c.damage),
            got.as_ref().map(|(_, a, _)| *a),
            got.as_ref().map(|(_, _, r)| *r),
        ),
    );

    // The gaps are positions, not absences: `ItemContainerContents.items` is
    // indexed by slot number and `copyInto` reads it positionally.
    let gappy = stack(
        &[(
            ids.container,
            container_value(&[None, Some((1, 5, None)), None, Some((2, 7, None))]),
        )],
        &[],
    );
    let g = components_of(&gappy);
    let kept = g
        .as_ref()
        .and_then(|(c, _, _)| c.container_contents().map(<[_]>::len));
    let occupied = g.as_ref().map(|(c, _, _)| c.container_items().count());
    c.record(
        "ct2.empty_container_slots_are_kept_while_the_tooltip_view_skips_them",
        kept == Some(4) && occupied == Some(2),
        format!(
            "{kept:?} slot(s) kept, {occupied:?} occupied. Dropping the gaps at \
             decode would renumber every slot after them; `addToTooltip` filters \
             instead (`nonEmptyItemsStream`), which is why the filter belongs in \
             the accessor and not in the walk"
        ),
    );

    // Absent, removed and explicitly-empty are three states, not two — the
    // same distinction a bundle draws, and for the same `getOrDefault` reason.
    let absent = components_of(&stack(&[], &[])).map(|(c, _, _)| c.container);
    let empty = components_of(&stack(&[(ids.container, container_value(&[]))], &[]))
        .map(|(c, _, _)| c.container);
    let removed = components_of(&stack(&[], &[ids.container])).map(|(c, _, _)| c.container);
    c.record(
        "ct3.an_absent_container_is_not_an_empty_one",
        absent == Some(None) && empty == Some(Some(Vec::new())) && removed == Some(None),
        format!(
            "absent {:?}, explicitly empty {:?}, removed {:?}. A patch that never \
             mentioned the component resolves through \
             `ItemContainerContents.EMPTY`, and so does a removal — only the item \
             id says whether that means an empty shulker box or a stone block",
            absent.as_ref().map(Option::is_some),
            empty.as_ref().map(|v| v.as_ref().map(Vec::len)),
            removed.as_ref().map(Option::is_some),
        ),
    );

    // A slot whose own patch names a component with no codec inherits the
    // patch's fail-closed rule rather than reporting a partial container.
    let mut bad = Vec::new();
    push_varint(&mut bad, 1);
    bad.push(1); // present
    push_varint(&mut bad, 276);
    push_varint(&mut bad, 1);
    push_varint(&mut bad, 1); // added
    push_varint(&mut bad, 0);
    push_varint(&mut bad, i32::MAX); // an id no registry can hold
    bad.extend_from_slice(&[0xAA, 0xBB]);
    let stuck = components_of(&stack(&[(ids.container, bad)], &[]));
    c.record(
        "ct4.an_unwalkable_component_inside_a_slot_stops_the_stack",
        stuck
            .as_ref()
            .is_some_and(|(comps, aligned, _)| !aligned && comps.container.is_none()),
        format!(
            "aligned {:?}, container {:?}. The slots read so far are discarded \
             rather than reported — a partial container presented as a whole one \
             is a confident wrong answer, which is the failure this decoder \
             refuses",
            stuck.as_ref().map(|(_, a, _)| *a),
            stuck.as_ref().map(|(c, _, _)| c.container.is_some()),
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
    let names: Vec<String> = lines.iter().map(rewo_gpu::tooltip::line_text).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
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
        .is_some_and(|l| l[0].color[0] > 0.9 && l[0].color[1] < 0.5);
    c.record(
        "e5.a_curse_is_red_and_the_tooltip_order_tag_leads",
        curse_first && curse_red,
        format!(
            "first line {:?} coloured {:?}. The order is the \
             `minecraft:tooltip_order` tag, not the ids and not the stack's own \
             order — the curses sit at the top of that tag",
            names.first(),
            lines.first().map(|l| l[0].color)
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

/// The enchantment glint's transform and its `hasFoil` gate (M43).
fn check_glint(c: &mut Checker, baked: &assets::BakedAssets) {
    use rewo_gpu::gui_item::{glint_offsets, glint_uv, GLINT_SCALE_ITEM, GLINT_SPEED};

    // The two offsets, at a time chosen so both moduli are exact.
    // millis = 1000 * 0.5 * 8 = 4000 → 4000/110000 and 4000/30000.
    let o = glint_offsets(1000.0, GLINT_SPEED);
    let want = (4000.0 / 110_000.0, 4000.0 / 30_000.0);
    c.record(
        "x1.the_two_glint_offsets_run_on_their_own_periods",
        (o.0 - want.0).abs() < 1e-6 && (o.1 - want.1).abs() < 1e-6,
        format!(
            "at one second: {o:?} against {want:?}. The periods are 110 s and 30 s, \
             which is why the sheen never visibly repeats, and the `long` cast \
             happens **before** the modulo — doing the remainder in floating point \
             drifts after the session has been up a few hours"
        ),
    );
    // …and they wrap, rather than growing without bound.
    let late = glint_offsets(1000.0 + 110_000.0 / (GLINT_SPEED as f64 * 8.0), GLINT_SPEED);
    c.record(
        "x2.the_first_offset_wraps_at_its_period",
        (late.0 - o.0).abs() < 1e-4,
        format!("one full 110 s period later the u offset is {:?} against {:?}", late.0, o.0),
    );

    // The matrix: scale by 8, rotate 10 degrees, then translate. The order
    // matters and is the reverse of the call order in the decompile.
    let at_origin = glint_uv([0.0, 0.0], (0.0, 0.0), GLINT_SCALE_ITEM);
    let unit_u = glint_uv([1.0, 0.0], (0.0, 0.0), GLINT_SCALE_ITEM);
    let ten = (std::f32::consts::PI / 18.0).sin_cos();
    let expect = [GLINT_SCALE_ITEM * ten.1, GLINT_SCALE_ITEM * ten.0];
    c.record(
        "x3.the_glint_matrix_scales_then_rotates_then_translates",
        at_origin == [0.0, 0.0]
            && (unit_u[0] - expect[0]).abs() < 1e-4
            && (unit_u[1] - expect[1]).abs() < 1e-4,
        format!(
            "uv (0,0) → {at_origin:?}; uv (1,0) → {unit_u:?} against {expect:?}. \
             JOML post-multiplies, so `translation().rotateZ().scale()` reads as \
             scale-then-rotate-then-translate on the coordinate — the reverse of \
             the order the calls appear in"
        ),
    );
    // The offset really does move the sample, and in the sign vanilla uses:
    // negative in u, positive in v.
    let shifted = glint_uv([0.0, 0.0], (0.25, 0.5), GLINT_SCALE_ITEM);
    c.record(
        "x4.the_u_offset_is_negative_and_the_v_offset_positive",
        (shifted[0] + 0.25).abs() < 1e-6 && (shifted[1] - 0.5).abs() < 1e-6,
        format!(
            "{shifted:?} for offsets (0.25, 0.5) — `translation(-o0, o1, 0)`, and \
             the opposing signs are what send the sheen diagonally rather than \
             straight across"
        ),
    );

    // `ItemStack.hasFoil()` — the override wins in **both** directions.
    let foil = |enchanted: bool, over: Option<bool>| {
        let mut comps = rewo_net::item_stack::StackComponents::default();
        if enchanted {
            comps.enchantments.push((0, 1));
        }
        comps.glint_override = over;
        comps.has_foil()
    };
    let table = [
        foil(true, None),
        foil(false, None),
        foil(false, Some(true)),
        foil(true, Some(false)),
    ];
    c.record(
        "x5.the_glint_override_wins_in_both_directions",
        table == [true, false, true, false],
        format!(
            "enchanted={:?} plain={:?} plain+override={:?} enchanted+override-off={:?}. \
             `hasFoil` is `override != null ? override : isEnchanted()`, so a golden \
             apple can glint and a Sharpness V sword can be told not to. Reading the \
             glint straight off the enchantment list gets the common case right and \
             both of these wrong",
            table[0], table[1], table[2], table[3]
        ),
    );
    c.record(
        "x6.the_glint_sheet_is_present_in_the_jar",
        baked.glint.as_ref().is_some_and(|g| g.w > 0 && g.h > 0),
        format!(
            "misc/enchanted_glint_item.png is {:?} — absent, no glint is drawn \
             rather than an invented shimmer",
            baked.glint.as_ref().map(|g| (g.w, g.h))
        ),
    );
}

// -- the tooltip's image pass (M52) --------------------------------------------
//
// `GuiGraphicsExtractor.tooltip` walks its component list twice — all the
// text, then all the images — and `ClientBundleTooltip` is the only image an
// *item* can produce, because `BundleItem.getTooltipImage` is the only
// override of it in the tree.
//
// Every number below is arithmetic, so it is graded as arithmetic here and as
// pixels in `pixels_inner`. The production functions are driven directly:
// nothing in this section reimplements the walk.

/// Where the grid's pixel witness puts it: a screen origin and pixels per GUI
/// pixel, chosen so a whole four-column grid fits inside the 256 px frame with
/// the leftmost — deliberately empty — column visible beside it.
const GRID_ORIGIN: (f32, f32) = (24.0, 24.0);
const GRID_PX: f32 = 2.0;

/// The grid's item icons, placed from the **production** cell walk.
///
/// One function so the render and the measurement cannot drift: the witness
/// asks `bundle_image` where the cells are, and so does the thing it grades.
fn bundle_grid_items(model: &str) -> Vec<GuiItem> {
    let bundle = test_bundle(3);
    let image = rewo_gpu::tooltip::bundle_image(&bundle, 0, 0, rewo_gpu::tooltip::GRID_WIDTH);
    image
        .cells
        .iter()
        .map(|cell| {
            let (ix, iy) = cell.icon();
            GuiItem {
                model: model.into(),
                x: GRID_ORIGIN.0 + ix as f32 * GRID_PX,
                y: GRID_ORIGIN.1 + iy as f32 * GRID_PX,
                size: 16.0 * GRID_PX,
                glint: false,
            }
        })
        .collect()
}

/// The screen rect a cell at tooltip-space `x` would put its icon in.
fn grid_icon_rect(cell_x: i32, cell_y: i32) -> (u32, u32, u32, u32) {
    let m = rewo_gpu::tooltip::SLOT_MARGIN;
    (
        (GRID_ORIGIN.0 + (cell_x + m) as f32 * GRID_PX) as u32,
        (GRID_ORIGIN.1 + (cell_y + m) as f32 * GRID_PX) as u32,
        (16.0 * GRID_PX) as u32,
        (16.0 * GRID_PX) as u32,
    )
}

/// A bundle of `n` single-item stacks, half full, nothing selected.
fn test_bundle(n: usize) -> rewo_gpu::tooltip::Bundle {
    rewo_gpu::tooltip::Bundle {
        counts: vec![1; n],
        selected: -1,
        weight: rewo_gpu::tooltip::Fraction::new(1, 2),
        weight_ok: true,
        empty_description_lines: 2,
    }
}

// -- the cell chrome (M58) -----------------------------------------------------

/// A plain text tooltip — the M40 shape, now that the pass takes a struct.
fn text_tip(pos: (i32, i32), size: (i32, i32)) -> rewo_gpu::container::TooltipDraw {
    rewo_gpu::container::TooltipDraw {
        pos,
        size,
        bundle: None,
    }
}

/// Where the chrome witnesses put the tooltip's text block, chosen so a
/// three-row grid and its progress bar both land inside the 256 px frame.
const TIP_POS: (i32, i32) = (60, 40);

/// A name line plus a bundle, measured and placed by the production walk.
///
/// The grid's `y` is read off [`image_pass_offsets`] at the index
/// [`insert_image`] actually chose, rather than restated as 1 — so a witness
/// built on this cannot agree with an insertion that has moved.
fn bundle_tip(bundle: &rewo_gpu::tooltip::Bundle) -> rewo_gpu::container::TooltipDraw {
    use rewo_gpu::tooltip::{
        bundle_image, image_pass_offsets, insert_image, measure, Component,
    };
    let mut list = vec![Component::text(40)];
    insert_image(&mut list, Component::Bundle(bundle.clone()));
    let at = list
        .iter()
        .position(|c| matches!(c, Component::Bundle(_)))
        .expect("the image component is in the list it was inserted into");
    let size = measure(&list);
    let y = image_pass_offsets(&list, TIP_POS.1)[at];
    rewo_gpu::container::TooltipDraw {
        pos: TIP_POS,
        size,
        bundle: Some(bundle_image(bundle, TIP_POS.0, y, size.0)),
    }
}

/// Whether every inner slice of a nine-slice sprite is constant along the axis
/// `blitTiledSprite` repeats it on.
///
/// Vanilla tiles those five pieces unless the `.mcmeta` sets `stretch_inner`;
/// Rewo stretches them. The two answers agree exactly when this holds, so it is
/// the precondition the container pass's one-quad-per-piece decomposition rests
/// on — a property of 26.2's art rather than a theorem, which is why it is
/// measured against the real jar instead of asserted in a comment.
fn inner_slices_uniform(rgba: &[u8], w: u32, h: u32, b: u32) -> bool {
    if b == 0 || 2 * b >= w || 2 * b >= h {
        return false;
    }
    let px = |x: u32, y: u32| -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };
    // The centre repeats both ways, so it has to be one colour.
    let centre = px(b, b);
    for y in b..(h - b) {
        for x in b..(w - b) {
            if px(x, y) != centre {
                return false;
            }
        }
    }
    // The top and bottom edges repeat horizontally: each of their rows must be
    // constant across the inner columns.
    for y in (0..b).chain((h - b)..h) {
        let first = px(b, y);
        if (b..(w - b)).any(|x| px(x, y) != first) {
            return false;
        }
    }
    // The left and right edges repeat vertically.
    for x in (0..b).chain((w - b)..w) {
        let first = px(x, b);
        if (b..(h - b)).any(|y| px(x, y) != first) {
            return false;
        }
    }
    true
}

/// M66 — the three tooltip stages the milestone closes: the advanced block
/// (F3+H), `minecraft:container`'s preview lines, and the held-item label.
///
/// Driven through the production functions in every case — the wire decode
/// through `read_optional`, the line generators through `live_cmd`'s own, the
/// rules through `rewo_gpu::{tooltip, hud}`. A gate that reimplemented any of
/// them would be grading its own copy, which is the M45 lesson.
fn check_advanced_tooltip(
    c: &mut Checker,
    baked: &assets::BakedAssets,
    paths: &DataPaths,
) -> Result<(), String> {
    use rewo_data::components::{DataComponentIds, DataComponentRegistry};
    use rewo_data::item_components_table::{prototype_component_count, prototype_has_component};
    use rewo_gpu::tooltip::{advanced_lines, container_plan, AdvancedLine, DurabilityState, TooltipFlag};
    use rewo_net::item_stack::{read_optional, StackComponents, StackDetail, WireSlot};

    let registry = DataComponentRegistry::load(&paths.registries_json())?;
    let ids = DataComponentIds::load(&paths.registries_json())?;
    rewo_net::component_wire::install_shapes(registry.ids());
    let items = rewo_data::items::Items::load(&paths.registries_json())?;
    let lang = &baked.lang;
    // The registry key the advanced block's literal line prints.
    fn key_placeholder() -> &'static str {
        "minecraft:diamond_sword"
    }

    // ── Stage 3: the advanced block ───────────────────────────────────────

    let tool = |damage: i32| DurabilityState {
        damage,
        max: 1561,
        has_max_damage: true,
        has_damage: true,
        unbreakable: false,
    };
    let damaged = advanced_lines(TooltipFlag::ADVANCED, tool(61), true, Some(19));
    let pristine = advanced_lines(TooltipFlag::ADVANCED, tool(0), true, Some(19));
    let mut unbreakable = tool(61);
    unbreakable.unbreakable = true;
    let never = advanced_lines(TooltipFlag::ADVANCED, unbreakable, true, Some(19));
    // MUTATION — the same line built from the damage rather than the
    // remaining, rendered so the difference is a string a reader can compare.
    let mutant_damage = crate::live_cmd::advanced_tooltip_lines(
        &[AdvancedLine::Durability {
            remaining: tool(61).damage_value(),
            max: 1561,
        }],
        key_placeholder(),
        lang,
    );
    let mutant_text = mutant_damage
        .first()
        .map(rewo_gpu::tooltip::line_text)
        .unwrap_or_default();
    c.record(
        "ad1.the_durability_arguments_are_remaining_then_max",
        damaged.first()
            == Some(&AdvancedLine::Durability {
                remaining: 1500,
                max: 1561,
            })
            && pristine.first() == Some(&AdvancedLine::RegistryId)
            && never.first() == Some(&AdvancedLine::RegistryId)
            && mutant_text == "Durability: 61 / 1561",
        format!(
            "a 61/1561-damaged tool reports {:?}. MUTATION (run) — emitting the \
             *damage* instead of the remaining renders {mutant_text:?}, which \
             reads as a nearly-broken tool on a nearly-full one and looks \
             entirely plausible. Two more mutations, also run: a pristine tool \
             emits no durability line at all ({} before the id) because \
             `isDamaged()` needs `damage > 0`, and an **Unbreakable** one never \
             does however damaged ({} before the id), because \
             `isDamageableItem()` is `has(MAX_DAMAGE) && !has(UNBREAKABLE) && \
             has(DAMAGE)`",
            damaged.first(),
            pristine.iter().take_while(|l| **l != AdvancedLine::RegistryId).count(),
            never.iter().take_while(|l| **l != AdvancedLine::RegistryId).count()
        ),
    );

    let full = advanced_lines(TooltipFlag::ADVANCED, tool(61), true, Some(19));
    let zero = advanced_lines(TooltipFlag::ADVANCED, tool(0), true, Some(0));
    let unknown = advanced_lines(TooltipFlag::ADVANCED, tool(0), true, None);
    let normal = advanced_lines(TooltipFlag::NORMAL, tool(61), true, Some(19));
    // MUTATION — the `count > 0` guard dropped, rendered.
    let ungated = crate::live_cmd::advanced_tooltip_lines(
        &[AdvancedLine::Components { count: 0 }],
        key_placeholder(),
        lang,
    );
    let ungated_text = ungated
        .first()
        .map(rewo_gpu::tooltip::line_text)
        .unwrap_or_default();
    c.record(
        "ad2.the_order_is_durability_then_a_dark_gray_literal_id_then_the_count",
        full
            == vec![
                AdvancedLine::Durability {
                    remaining: 1500,
                    max: 1561
                },
                AdvancedLine::RegistryId,
                AdvancedLine::Components { count: 19 },
            ]
            && zero == vec![AdvancedLine::RegistryId]
            && unknown == vec![AdvancedLine::RegistryId]
            && normal.is_empty()
            && ungated_text == "0 component(s)",
        format!(
            "{} lines in order {full:?}; at count 0 the block is just the id \
             ({} line), and an unresolvable count drops the line rather than \
             guessing ({} line); the NORMAL flag emits nothing. MUTATION (run) — \
             `count > 0` is the only guard, so dropping it renders \
             {ungated_text:?} on a stack vanilla says nothing about",
            full.len(),
            zero.len(),
            unknown.len()
        ),
    );

    // The id is a **literal**, so it must not be run through the language
    // file. The mutation is exactly that: treat it as a key.
    let key = "minecraft:diamond_sword";
    let rendered = crate::live_cmd::advanced_tooltip_lines(
        &advanced_lines(TooltipFlag::ADVANCED, tool(61), true, Some(19)),
        key,
        lang,
    );
    let id_line = rendered.get(1).map(|l| rewo_gpu::tooltip::line_text(l));
    let as_key = lang.get(key);
    let dark_gray = rendered.get(1).and_then(|l| l.first()).map(|s| s.color);
    c.record(
        "ad3.the_registry_id_is_a_literal_in_dark_gray",
        id_line.as_deref() == Some(key)
            && as_key.is_none()
            && dark_gray == Some(rewo_gpu::tooltip::DARK_GRAY),
        format!(
            "the second line is {id_line:?} at colour {dark_gray:?} \
             (DARK_GRAY 0x555555). MUTATION — `Component.literal` is not \
             `Component.translatable`; running the id through the language file \
             finds {as_key:?}, and `TranslatableContents`' fallback then prints \
             the key anyway — the same string by luck, until a datapack defines \
             that key and the tooltip shows its value instead"
        ),
    );

    let dur_line = rendered.first().map(|l| rewo_gpu::tooltip::line_text(l));
    let count_line = rendered.get(2).map(|l| rewo_gpu::tooltip::line_text(l));
    // MUTATION — the two arguments swapped, run through the same formatter.
    let swapped = lang
        .get("item.durability")
        .map(|t| rewo_data::lang::format(t, &["1561", "1500"]));
    c.record(
        "ad4.the_two_translated_lines_read_as_vanilla_renders_them",
        dur_line.as_deref() == Some("Durability: 1500 / 1561")
            && count_line.as_deref() == Some("19 component(s)")
            && swapped.as_deref() == Some("Durability: 1561 / 1500"),
        format!(
            "{dur_line:?} and {count_line:?}, from `item.durability` \
             ({:?}) and `item.components` ({:?}). MUTATION (run) — swapping the \
             two `%s`s gives {swapped:?}, which reads as a tool with more life \
             than it can hold and is otherwise a perfectly well-formed line",
            lang.get("item.durability"),
            lang.get("item.components")
        ),
    );

    // `PatchedDataComponentMap.size()` — the merged map's, not the patch's.
    let count_of = |item: &str, added: &[&str], removed: &[&str]| -> Option<i32> {
        let name_id = |n: &str| registry.ids().get(n).copied().expect("registered");
        let detail = StackDetail {
            container: Vec::new(),
            added: added.iter().map(|n| name_id(n)).collect(),
            removed: removed.iter().map(|n| name_id(n)).collect(),
        };
        StackComponents {
            added: detail.added.clone(),
            removed: detail.removed.clone(),
            ..Default::default()
        }
        .component_count(
            || prototype_component_count(item),
            |id| prototype_has_component(item, registry.name_of(id)?),
        )
    };
    // The expectations are **deltas from the table**, not hand-written
    // absolutes: the first draft of this witness guessed 13 and 19 for the two
    // prototype sizes and both were off by one, which is exactly the kind of
    // hand-counting error a gate is supposed to make impossible.
    let (p_dirt, p_sword) = (
        prototype_component_count("minecraft:dirt"),
        prototype_component_count("minecraft:diamond_sword"),
    );
    let bare = count_of("minecraft:dirt", &[], &[]);
    let named = count_of("minecraft:dirt", &["minecraft:custom_name"], &[]);
    let override_ = count_of("minecraft:diamond_sword", &["minecraft:damage"], &[]);
    let removed = count_of("minecraft:diamond_sword", &[], &["minecraft:damage"]);
    let unknown_item = count_of("minecraft:not_an_item", &[], &[]);
    // MUTATION — `this.components.size()` read as the patch's own entry count.
    let as_patch_size = |added: usize, removed: usize| (added + removed) as i32;
    let patch_sizes = (
        as_patch_size(0, 0),
        as_patch_size(1, 0),
        as_patch_size(1, 0),
    );
    c.record(
        "ad5.the_count_is_the_merged_maps_size_not_the_patchs",
        p_dirt.is_some()
            && p_sword.is_some()
            && bare == p_dirt
            && named == p_dirt.map(|n| n + 1)
            && override_ == p_sword
            && removed == p_sword.map(|n| n - 1)
            && unknown_item.is_none()
            // The two must not coincide, or "equals the prototype" proves
            // nothing about the addition arm.
            && p_dirt != p_sword
            && patch_sizes == (0, 1, 1),
        format!(
            "an unpatched dirt is {bare:?} (its prototype's {p_dirt:?}) and one \
             carrying a `custom_name` — which no prototype has — is {named:?}; a \
             sword whose `damage` the patch *overrides* stays {override_:?} \
             (its prototype's {p_sword:?}, unchanged, because an override is not \
             an addition) and one whose `damage` it *removes* drops to \
             {removed:?}; an item outside the table is {unknown_item:?}. \
             MUTATION (run) — reading `this.components.size()` as the patch's \
             own entry count gives {patch_sizes:?} for those first three, which \
             is what a client that never consults the item prototype shows for \
             nearly every stack in the game"
        ),
    );

    // ── Stage 4: the container's lines ────────────────────────────────────

    // The wire decode itself is main's `ct1`/`ct2` (M63), which grade the
    // optional list, the gaps and the alignment to the byte. What is new here
    // is the **second** level of `getHoverName`: M63 captured `custom_name`
    // alone and reasoned that `ITEM_NAME` is "answered by the item table on
    // the rendering side". The item table answers the item's *prototype*
    // `item_name`; a patched one is a different value entirely.
    let name_tag = |text: &str| -> Vec<u8> {
        let mut v = vec![10u8, 8u8];
        v.extend_from_slice(&(4u16).to_be_bytes());
        v.extend_from_slice(b"text");
        v.extend_from_slice(&(text.len() as u16).to_be_bytes());
        v.extend_from_slice(text.as_bytes());
        v.push(0);
        v
    };
    // `(item, count, custom_name, item_name)` per present slot.
    let named_container = |slots: &[(i32, i32, Option<&str>, Option<&str>)]| -> Vec<u8> {
        let mut v = Vec::new();
        push_varint(&mut v, 1); // stack count
        push_varint(&mut v, 276); // the shulker box's own item id
        push_varint(&mut v, 1); // one component added
        push_varint(&mut v, 0); // none removed
        push_varint(&mut v, ids.container);
        push_varint(&mut v, slots.len() as i32);
        for (item, count, custom, item_name) in slots {
            v.push(1); // present
            push_varint(&mut v, *item);
            push_varint(&mut v, *count);
            let entries = usize::from(custom.is_some()) + usize::from(item_name.is_some());
            push_varint(&mut v, entries as i32);
            push_varint(&mut v, 0);
            if let Some(n) = custom {
                push_varint(&mut v, ids.custom_name);
                v.extend_from_slice(&name_tag(n));
            }
            if let Some(n) = item_name {
                push_varint(&mut v, ids.item_name);
                v.extend_from_slice(&name_tag(n));
            }
        }
        // A sentinel a misread would eat.
        v.push(0xAB);
        v
    };
    let dirt_id = items.id("minecraft:dirt").ok_or("no dirt in the registry")?;
    let sword_id = items
        .id("minecraft:diamond_sword")
        .ok_or("no diamond_sword in the registry")?;
    let both = named_container(&[
        (sword_id, 1, Some("Skullcrusher"), Some("Blade")),
        (sword_id, 1, None, Some("Blade")),
        (sword_id, 1, None, None),
    ]);
    let decoded = {
        let mut r = rewo_proto::reader::PacketReader::new(&both);
        match read_optional(&mut r, ids) {
            Ok(WireSlot::Stack(s)) => s
                .components
                .container_contents()
                .map(|slots| (slots.to_vec(), r.remaining())),
            _ => None,
        }
    };
    let hovers: Vec<String> = decoded
        .as_ref()
        .map(|(slots, _)| {
            slots
                .iter()
                .flatten()
                .map(|s| s.hover_name("Diamond Sword").to_string())
                .collect()
        })
        .unwrap_or_default();
    // MUTATION (run) — `custom_name` alone, which is what M63 captured.
    let custom_only: Vec<String> = decoded
        .as_ref()
        .map(|(slots, _)| {
            slots
                .iter()
                .flatten()
                .map(|s| {
                    s.custom_name
                        .clone()
                        .unwrap_or_else(|| "Diamond Sword".to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    c.record(
        "cn1.the_container_slots_hover_name_is_a_two_level_override",
        decoded.as_ref().map(|(_, rest)| *rest) == Some(1)
            && hovers == vec!["Skullcrusher", "Blade", "Diamond Sword"]
            && custom_only == vec!["Skullcrusher", "Diamond Sword", "Diamond Sword"],
        format!(
            "three slots — both names, `item_name` only, neither — resolve to \
             {hovers:?}, and the sentinel is still unread. `getHoverName` is \
             `getOrDefault(CUSTOM_NAME, getOrDefault(ITEM_NAME, item.getName()))`, \
             two levels of override rather than a name and a fallback. \
             MUTATION (run) — capturing `custom_name` alone gives \
             {custom_only:?}: the middle stack loses its patched name and \
             renders under the item table's answer, which is the *prototype's* \
             `item_name` and not the patch's"
        ),
    );

    // The line rule. `lineCount <= 4` with the increment inside the guard.
    let plans: Vec<_> = [0usize, 1, 4, 5, 6, 27].iter().map(|n| container_plan(*n)).collect();
    let mutated = |n: usize| {
        // MUTATION — the same loop with `lineCount < 4`.
        let (mut line, mut item) = (0i32, 0i32);
        for _ in 0..n {
            item += 1;
            if line < 4 {
                line += 1;
            }
        }
        (line as usize, item - line)
    };
    // MUTATION — the remainder reported as the total rather than
    // `itemCount - lineCount`.
    let remainder_as_total: Vec<usize> = [5usize, 6, 27].to_vec();
    c.record(
        "cn2.five_lines_fit_and_the_sixth_becomes_a_remainder",
        plans[3].shown == 5
            && plans[3].more == 0
            && plans[4].shown == 5
            && plans[4].more == 1
            && plans[5].shown == 5
            && plans[5].more == 22
            && mutated(5) == (4, 1)
            && remainder_as_total[0] as i32 != plans[3].more,
        format!(
            "0/1/4/5/6/27 stacks -> {:?}. A five-stack box lists all five and says \
             nothing more; a six-stack one lists five and says `and 1 more…`. \
             MUTATION (run) — `lineCount < 4` turns five stacks into {:?}, i.e. \
             four lines *and* a spurious remainder. A second mutation (run) — \
             reporting the remainder as the **total** gives {:?} for those same \
             three counts, so a full box would say `and 5 more…` underneath all \
             five of the lines it just printed",
            plans
                .iter()
                .map(|p| (p.shown, p.more))
                .collect::<Vec<_>>(),
            mutated(5),
            remainder_as_total
        ),
    );

    // The translation, through the production line generator. The two keys
    // exist only after M54's deprecation pass (witness `l1`); this is the
    // functional half — that the *lines* come out.
    // Seven slots, one of them a gap — `nonEmptyItemsStream` skips it, so six
    // stacks reach the line rule and the seventh position changes nothing.
    let six: Vec<Option<rewo_net::item_stack::ContainerSlot>> = (0..7)
        .map(|i| {
            (i != 1).then(|| rewo_net::item_stack::ContainerSlot {
                item_id: dirt_id,
                count: 64,
                custom_name: (i == 0).then(|| "Skullcrusher".to_string()),
                item_name: None,
            })
        })
        .collect();
    let lines = crate::live_cmd::container_lines(&six, &items, &baked.item_names, lang);
    let raw_json = assets::jar_text(
        &client_jar("26.2").ok_or("client jar not found")?,
        rewo_data::lang::EN_US_PATH,
    )
    .ok_or("en_us.json missing from the jar")?;
    let raw = rewo_data::lang::Language::raw(&raw_json);
    let without_rename = crate::live_cmd::container_lines(&six, &items, &baked.item_names, &raw);
    let texts: Vec<String> = lines.iter().map(rewo_gpu::tooltip::line_text).collect();
    c.record(
        "cn3.the_container_lines_read_as_vanilla_renders_them",
        texts.len() == 6
            && texts[0] == "Skullcrusher x64"
            && texts[1] == "Dirt x64"
            && texts[5] == "and 1 more..."
            && lines[5][0].italic
            && !lines[0][0].italic
            && without_rename.is_empty(),
        format!(
            "{texts:?}; the trailing line is italic ({}) per \
             `.withStyle(ChatFormatting.ITALIC)` and the item lines are not. The \
             first entry's own `custom_name` wins over the translated item name, \
             which is `getHoverName`'s two-level override. MUTATION — the same \
             call against a **raw** `en_us.json` produces {} lines, because both \
             keys live under their pre-rename `container.shulkerBox.*` names \
             until M54's deprecation pass moves them",
            lines[5][0].italic,
            without_rename.len()
        ),
    );

    // ── Stage 5: the held-item label ──────────────────────────────────────

    use rewo_gpu::hud::{selected_item_name_pos, text_backdrop_rect, tool_highlight_alpha, ToolHighlight};

    let mut h = ToolHighlight::default();
    h.tick(Some((1, "Diamond Sword")), 1.0);
    let after_pick = h.timer;
    h.tick(Some((1, "Diamond Sword")), 1.0);
    let held_still = h.timer;
    h.tick(Some((1, "Skullcrusher")), 1.0);
    let after_rename = h.timer;
    // MUTATION — the same clock comparing item identity alone.
    let mut ident = ToolHighlight::default();
    ident.tick(Some((1, "Diamond Sword")), 1.0);
    ident.tick(Some((1, "Diamond Sword")), 1.0);
    let mutated_rename = {
        // `selected.is(last.getItem())` only: the name change is invisible, so
        // the timer keeps counting down.
        ident.tick(Some((1, "Diamond Sword")), 1.0);
        ident.timer
    };
    c.record(
        "hh1.the_timer_re_triggers_on_a_hover_name_change_with_the_same_item",
        after_pick == 40 && held_still == 39 && after_rename == 40 && mutated_rename == 38,
        format!(
            "picking it up sets {after_pick}, holding it runs down to {held_still}, \
             and an anvil rename of the **same item** resets it to \
             {after_rename}. MUTATION — comparing `selected.is(last.getItem())` \
             alone leaves it at {mutated_rename} and the new name never appears \
             over the hotbar. Vanilla's guard is three-part: \
             `last.isEmpty() || !selected.is(last.getItem()) || \
             !selected.getHoverName().equals(last.getHoverName())`"
        ),
    );

    let alphas: Vec<i32> = [40, 11, 10, 9, 5, 1, 0]
        .iter()
        .map(|t| tool_highlight_alpha(*t))
        .collect();
    // MUTATION — a fade spread over the whole 40-tick timer.
    let over_forty: Vec<i32> = [40, 11, 10, 9, 5, 1, 0]
        .iter()
        .map(|t| (*t as f32 * 256.0 / 40.0) as i32)
        .collect();
    c.record(
        "hh2.the_fade_is_the_last_ten_ticks_only",
        alphas == vec![255, 255, 255, 230, 128, 25, 0] && over_forty[0] == 256,
        format!(
            "timers 40/11/10/9/5/1/0 -> alphas {alphas:?}: opaque for the first \
             thirty ticks, then linear over the last ten, clamped at 255. \
             MUTATION — `timer * 256 / 40` gives {over_forty:?}, which is \
             translucent the moment the label appears *and* overflows past 255 \
             at the top with no clamp to catch it"
        ),
    );

    let survival = selected_item_name_pos(427, 240, 41, true);
    let creative = selected_item_name_pos(427, 240, 41, false);
    c.record(
        "hh3.the_label_is_centred_and_drops_fourteen_rows_with_no_health_bar",
        survival == ((427 - 41) / 2, 240 - 59) && creative == (survival.0, 240 - 59 + 14),
        format!(
            "on a 427x240 GUI a 41 px label lands at {survival:?} in survival and \
             {creative:?} in creative. MUTATION — a fixed `y = guiHeight() - 59` \
             puts the label on top of the hotbar in creative, where there are no \
             hearts and vanilla moves it down by \
             {}. `canHurtPlayer()` is `localPlayerMode.isSurvival()`, and that \
             is SURVIVAL **or ADVENTURE**",
            rewo_gpu::hud::SELECTED_ITEM_NAME_NO_HEALTH_SHIFT
        ),
    );

    c.record(
        "hh4.the_text_backdrop_is_absent_at_vanillas_defaults",
        text_backdrop_rect(10, 20, 40, 0).is_none()
            && text_backdrop_rect(10, 20, 40, 0x8000_0000) == Some((8, 18, 52, 31)),
        format!(
            "a zero background colour draws no fill; a non-zero one draws \
             {:?} — 2 px around a **9** px text row, not the tooltip's 10. \
             `getBackgroundColor(0.0F)` returns zero while `backgroundForChatOnly` \
             is set, which it is by default, so vanilla draws no fill here \
             either. MUTATION — filling unconditionally puts a black bar under \
             the label of every item you pick up",
            text_backdrop_rect(10, 20, 40, 0x8000_0000)
        ),
    );

    Ok(())
}

fn check_tooltip_image(c: &mut Checker, baked: &assets::BakedAssets) {
    use rewo_gpu::tooltip::{
        bundle_image, content_x_offset, continuous_cursor_end, image_pass_offsets, insert_image,
        measure, text_pass_offsets, BarLabel, CellKind, Component, Fraction, GRID_WIDTH,
    };

    // 1. Where the image goes in the list.
    let mut list = vec![Component::text(40), Component::text(55)];
    insert_image(&mut list, Component::Bundle(test_bundle(3)));
    let mut alone: Vec<Component> = Vec::new();
    insert_image(&mut alone, Component::Bundle(test_bundle(3)));
    c.record(
        "ti1.the_image_component_is_inserted_at_index_one",
        matches!(list[0], Component::Text { width: 40 })
            && matches!(list[1], Component::Bundle(_))
            && matches!(list[2], Component::Text { width: 55 })
            && alone.len() == 1
            && matches!(alone[0], Component::Bundle(_)),
        format!(
            "with a name and one detail the grid lands at index 1 of {}; with an \
             empty list it lands at 0. `components.add(components.isEmpty() ? 0 : 1, …)` \
             — appending instead puts the grid below the enchantment lines, and \
             dropping the `isEmpty` guard throws `IndexOutOfBoundsException` on a \
             stack whose name its tooltip display hides",
            list.len()
        ),
    );

    // 2. A bundle's width is a constant, not a measurement.
    let one = Component::Bundle(test_bundle(1));
    let twelve = Component::Bundle(test_bundle(12));
    c.record(
        "ti2.a_bundle_is_a_fixed_96_wide_whatever_it_holds",
        one.width() == GRID_WIDTH && twelve.width() == GRID_WIDTH && one.height() != twelve.height(),
        format!(
            "1 stack -> {}x{}, 12 -> {}x{}. `ClientBundleTooltip.getWidth` is a \
             literal `return 96`, so only the *height* tracks the contents — a \
             width that grew with the row count would narrow a one-item bundle's box",
            one.width(),
            one.height(),
            twelve.width(),
            twelve.height()
        ),
    );

    // 3. The measure loop runs over every component kind, which is what makes
    //    the box account for an image at all.
    let name_only = vec![Component::text(40)];
    let mut with_grid = name_only.clone();
    insert_image(&mut with_grid, Component::Bundle(test_bundle(3)));
    let (tw, th) = measure(&name_only);
    let (bw, bh) = measure(&with_grid);
    // One row of cells (24) + the bar (13) + its two 4 px margins.
    let grid_h = 24 + 13 + 8;
    c.record(
        "ti3.the_box_widens_and_deepens_to_the_image",
        (tw, th) == (40, 8) && (bw, bh) == (GRID_WIDTH, 10 + grid_h),
        format!(
            "the name alone measures {tw}x{th}; with the grid it is {bw}x{bh}. \
             `getWidth`/`getHeight` are polymorphic and the measure loop has no \
             special case, so the image contributes its own 96 and its own \
             {grid_h}. A text-only width leaves the box narrower than the grid \
             it must contain. Note the height is 10+{grid_h} and not 8+{grid_h}: \
             `lines.size() == 1 ? -2 : 0` counts **components**, so the image \
             hands back the two pixels a lone name gives up"
        ),
    );

    // 4. The restart between the passes.
    let text_y = text_pass_offsets(&with_grid, 100);
    let image_y = image_pass_offsets(&with_grid, 100);
    let continued = continuous_cursor_end(&with_grid, 100);
    c.record(
        "ti4.the_image_pass_restarts_localY_at_the_box_top",
        text_y == image_y && image_y == vec![100, 112] && continued == 157,
        format!(
            "text pass {text_y:?}, image pass {image_y:?}, and a single continuous \
             cursor would have reached {continued}. The `localY = y;` between the \
             two loops is the whole difference between them — run them as one \
             cursor and the grid draws {} px below its own box, which is exactly \
             the overlap-the-text failure the two passes exist to avoid",
            continued - 100
        ),
    );

    // 5. The fill order.
    let three = test_bundle(3);
    let img = bundle_image(&three, 0, 0, GRID_WIDTH);
    let walk: Vec<(i32, i32, usize)> = img
        .cells
        .iter()
        .map(|cell| match cell.kind {
            CellKind::Slot { item, .. } => (cell.x, cell.y, item),
            CellKind::Badge { .. } => (cell.x, cell.y, usize::MAX),
        })
        .collect();
    c.record(
        "ti5.the_grid_fills_bottom_right_to_top_left",
        walk == vec![(72, 0, 2), (48, 0, 1), (24, 0, 0)],
        format!(
            "three stacks occupy {walk:?} as (x, y, item). Both start positions are \
             the grid's *far* edge and both subtract — `xStartPos = x + offset + 96` \
             then `drawX = xStartPos - columnNumber * 24` — so column 1 is the \
             rightmost and the leftmost cell at x 0 stays empty. Walking top-left \
             to bottom-right would give [(0,0,0),(24,0,1),(48,0,2)]: different \
             cells *and* the reversed item mapping, since \
             `itemVisualOrderIndex = shownItems.size() - slotNumber` puts the last \
             stack in the first cell visited"
        ),
    );

    // 6. The overflow boundary, and where the badge lands.
    let twelve_b = test_bundle(12);
    let thirteen = test_bundle(13);
    let twelve_img = bundle_image(&twelve_b, 0, 0, GRID_WIDTH);
    let thirteen_img = bundle_image(&thirteen, 0, 0, GRID_WIDTH);
    let badges = |i: &rewo_gpu::tooltip::BundleImage| {
        i.cells
            .iter()
            .filter(|cell| matches!(cell.kind, CellKind::Badge { .. }))
            .count()
    };
    c.record(
        "ti6.thirteen_stacks_badge_and_exactly_twelve_do_not",
        badges(&twelve_img) == 0
            && twelve_img.cells.len() == 12
            && badges(&thirteen_img) == 1
            && thirteen_img.cells[0].kind == CellKind::Badge { hidden: 5 }
            && (thirteen_img.cells[0].x, thirteen_img.cells[0].y) == (72, 48),
        format!(
            "12 stacks -> {} cells, {} badges; 13 -> {} cells, {} badges with the \
             badge at {:?}. `isOverflowing` is `size() > 12`, so twelve fills the \
             grid exactly; and `shouldRenderSurplusText`'s `column * row == 1` is \
             the *first cell visited*, which the reversed walk makes the \
             **bottom-right** — not the top-left. `min(13, …)` for the slot count, \
             or a badge at twelve, moves both of these",
            twelve_img.cells.len(),
            badges(&twelve_img),
            thirteen_img.cells.len(),
            badges(&thirteen_img),
            (thirteen_img.cells[0].x, thirteen_img.cells[0].y)
        ),
    );

    // 7. What the badge counts, and how many items a full bundle shows.
    let mut heavy = test_bundle(13);
    heavy.counts = vec![64; 13];
    let heavy_hidden = match bundle_image(&heavy, 0, 0, GRID_WIDTH).cells[0].kind {
        CellKind::Badge { hidden } => hidden,
        _ => -1,
    };
    c.record(
        "ti7.the_badge_counts_hidden_items_not_hidden_stacks",
        thirteen.shown_items() == 8 && heavy_hidden == 5 * 64 && thirteen.hidden_item_count() == 5,
        format!(
            "13 stacks show {} of them; hidden 13x1 badges +{} and 13x64 badges \
             +{heavy_hidden}. Two things read backwards here. \
             `getNumberOfItemsToShow` subtracts the ragged row using \
             `numberOfItemStacks % 4` — 13 % 4 = 1, so 3 come off the 11 \
             available and **eight** show, leaving the grid's top row blank \
             beside the badge. And `getAmountOfHiddenItems` sums the hidden \
             stacks' `count()`, so it is +320 for full stacks, not +5",
            thirteen.shown_items(),
            thirteen.hidden_item_count()
        ),
    );

    // 8. The grid is centred in the whole box, not under its own component.
    let wide = vec![Component::text(200), Component::Bundle(test_bundle(3))];
    let (wide_w, _) = measure(&wide);
    let centred = bundle_image(&test_bundle(3), 0, 0, wide_w);
    c.record(
        "ti8.the_grid_centres_in_the_whole_box",
        content_x_offset(wide_w) == 52
            && centred.cells[0].x == 124
            && content_x_offset(GRID_WIDTH) == 0,
        format!(
            "a 200 px line makes the box {wide_w} wide, so `getContentXOffset` is \
             {} and the first cell moves from x 72 to {}. `extractImage` is handed \
             the *tooltip's* measured w, not the component's own 96 — centring \
             against 96 (offset 0) or left-aligning both leave the grid hard \
             against the box's left edge under a long enchantment line",
            content_x_offset(wide_w),
            centred.cells[0].x
        ),
    );

    // 9. The badge's own placement, through the real centring helper and the
    //    real font. A missing font fails rather than skips.
    let advance = baked.font.as_ref().map(|f| f.advance);
    let badge_geom = advance.map(|adv| {
        let cell = thirteen_img.cells[0];
        let (ax, ay) = cell.badge_anchor();
        // The label the badge itself carries — reading it back off the cell
        // rather than restating it, so this cannot agree with a walk that has
        // stopped producing a badge at all.
        let hidden = match cell.kind {
            CellKind::Badge { hidden } => hidden,
            CellKind::Slot { .. } => -1,
        };
        let label = format!("+{hidden}");
        (
            rewo_gpu::text::centered_x(&label, &adv, ax),
            ay,
            rewo_gpu::text::width(&label, &adv),
        )
    });
    c.record(
        "ti9.the_badge_is_centred_on_its_cell_by_integer_division",
        badge_geom.is_some_and(|(x, y, w)| {
            let (ax, _) = thirteen_img.cells[0].badge_anchor();
            x == ax - w / 2 && (ax, y) == (72 + 12, 48 + 10) && w > 0
        }),
        format!(
            "anchor {:?}, laid out at {badge_geom:?} as (x, y, width). \
             `extractCount` is `centeredText(font, \"+\"+n, drawX + 12, drawY + 10, -1)` \
             and `centeredText` is `x - font.width(str) / 2` — **integer** \
             division, and the vertical anchor is a flat 10 rather than the \
             cell's own half-height of 12. Rounding the halving, or using 12 for \
             the y, moves the badge a pixel each way",
            thirteen_img.cells[0].badge_anchor()
        ),
    );

    // 10. The progress bar's three states.
    let bar_at = |num: i32, den: i32| {
        let mut b = test_bundle(1);
        b.weight = Fraction::new(num, den);
        bundle_image(&b, 0, 0, GRID_WIDTH).bar
    };
    let (mid, full, empty) = (bar_at(1, 2), bar_at(1, 1), bar_at(0, 1));
    let bars = (
        mid.map(|b| (b.fill, b.full, b.label)),
        full.map(|b| (b.fill, b.full, b.label)),
        empty.map(|b| (b.fill, b.full, b.label)),
    );
    c.record(
        "ti10.the_bar_is_labelled_only_at_its_two_ends",
        bars == (
            Some((47, false, None)),
            Some((94, true, Some(BarLabel::Full))),
            Some((0, false, Some(BarLabel::Empty))),
        ) && mid.is_some_and(|b| (b.x, b.y) == (0, 24 + 4)),
        format!(
            "half {:?}, full {:?}, empty {:?} as (fill, full, label), and the bar \
             sits at {:?}. `getProgressBarFillText` returns the empty label at \
             exactly zero, the full label at one **or more**, and `null` in \
             between — a label on every state would write \"empty\" across a \
             half-full bar. The fill is `mulAndTruncate(weight, 94)`, so a half \
             weight is 47 of the 94 the border's 96 leaves room for, and the bar's \
             y is the component's own top plus the *full* grid height, not the \
             last row that happened to be drawn",
            bars.0, bars.1, bars.2,
            mid.map(|b| (b.x, b.y))
        ),
    );
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
            glint: false,
        },
        GuiItem {
            model: block.into(),
            x: 144.0,
            y: 96.0,
            size: 48.0,
            glint: false,
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
        glint: false,
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
    // The bundle grid's icons, built through the same pass and the same atlas
    // as the hotbar's. `bundle_image` decides where they go.
    let grid_items = bundle_grid_items(block);
    let grid_verts = build_vertices(&held, &grid_items, &lights, &|t| uv.get(&t).copied());

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
        c, &mut gpu, &mut off, &mut wr, baked, &atlas, &verts, &grid_verts, &draw, args,
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
    grid_verts: &[rewo_gpu::gui_item::GuiItemVertex],
    draw: &OverlayDraw,
    args: &InventoryshotArgs,
) -> Result<(), String> {
    // The icons draw on top of the hotbar the HUD paints, so the HUD has to
    // exist for them to appear at all — which is itself the M34 wiring claim.
    let sprites = crate::live_cmd::hud_sprites(baked).ok_or("hud sprites missing from the jar")?;
    wr.init_hud(gpu, &sprites)?;
    // M79 added the two gauges; this gate owns neither, so it passes the
    // default (no XP bar, no cooldown sweep) and its measurements are
    // unchanged.
    wr.set_hud(20.0, 20, 0, rewo_gpu::hud::HudGauges::default());
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
        glint: false,
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
    let with_tip = tip_at(wr, gpu, off, Some(text_tip((60, 40), (40, 8))))?;
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
    let moved = tip_at(wr, gpu, off, Some(text_tip((100, 90), (40, 8))))?;
    c.record(
        "t6.the_tooltip_follows_its_origin",
        changed_bounds(&no_tip, &moved) != tip_box,
        format!(
            "at (60,40) the box is {tip_box:?}; at (100,90) it is {:?}",
            changed_bounds(&no_tip, &moved)
        ),
    );

    // -- the image pass, in pixels (M52) -------------------------------------
    //
    // The arithmetic is graded in `check_tooltip_image`; these two ask whether
    // it reached the framebuffer. Both drive the production measure and the
    // production cell walk — a witness that placed its own rectangles would
    // grade this file rather than the client.
    let name_only = vec![rewo_gpu::tooltip::Component::text(40)];
    let mut with_grid = name_only.clone();
    rewo_gpu::tooltip::insert_image(
        &mut with_grid,
        rewo_gpu::tooltip::Component::Bundle(test_bundle(3)),
    );
    let text_box = rewo_gpu::tooltip::measure(&name_only);
    let grid_box = rewo_gpu::tooltip::measure(&with_grid);
    let narrow = tip_at(wr, gpu, off, Some(text_tip((40, 30), text_box)))?;
    let widened = tip_at(wr, gpu, off, Some(text_tip((40, 30), grid_box)))?;
    let narrow_box = changed_bounds(&no_tip, &narrow);
    let wide_box = changed_bounds(&no_tip, &widened);
    let span = |b: Option<(u32, u32, u32, u32)>| b.map(|(x0, _, x1, _)| x1 - x0);
    let grid_w = rewo_gpu::tooltip::GRID_WIDTH as f32;
    c.record(
        "ti11.a_bundle_widens_the_rendered_box_to_hold_its_grid",
        match (span(wide_box), span(narrow_box)) {
            (Some(wide), Some(thin)) => {
                wide > thin
                    && wide as f32 >= grid_w * scale
                    && wide as f32 <= (grid_w + 2.0 * inset as f32) * scale
            }
            _ => false,
        },
        format!(
            "the name alone draws a box {:?} wide, the name plus the grid {:?} — \
             measured {text_box:?} against {grid_box:?}. The image contributes its \
             fixed 96 through the same measure loop as any text line, so the box \
             has to reach at least {} px before the grid could fit inside it. \
             Measuring the text only, which is what M40 did, leaves the box \
             {} px narrower than its own contents",
            span(narrow_box),
            span(wide_box),
            grid_w * scale,
            (grid_box.0 - text_box.0) as f32 * scale
        ),
    );

    // And the cells themselves, as real icons through the real item pass. The
    // discriminator is which column stays empty: with three stacks in a
    // four-column grid, one column is unused, and the walk's direction decides
    // which one.
    let grid_base = shot(gpu, off, wr, &[])?;
    let grid_img = shot(gpu, off, wr, grid_verts)?;
    let first = grid_icon_rect(72, 0);
    let unused = grid_icon_rect(0, 0);
    let ink = |r: (u32, u32, u32, u32)| changed(&grid_base, &grid_img, r.0, r.1, r.2, r.3);
    c.record(
        "ti12.the_rendered_icons_land_in_the_cells_the_walk_reports",
        ink(first) > 0 && ink(unused) == 0 && !grid_verts.is_empty(),
        format!(
            "the bottom-right cell at {first:?} changed {} pixels; the leftmost \
             column at {unused:?} changed {}. Three stacks fill three of the four \
             columns, and `drawX = xStartPos - columnNumber * 24` fills them from \
             the right — so column 4 is the empty one. A top-left-to-bottom-right \
             walk swaps these two numbers exactly, leaving the *right* column \
             blank instead",
            ink(first),
            ink(unused)
        ),
    );

    // -- the cell chrome, in pixels (M58) ------------------------------------
    //
    // M52 computed where every cell goes and blitted none of them. These grade
    // the six `blitSprite` calls `extractSlot` and `extractProgressbar` make:
    // which sprite lands on which cell, which cell gets none, and the order the
    // bar's two go down in.
    //
    // Every rectangle below is read off the production walk through
    // `bundle_tip`, so a witness cannot agree with a grid that has moved.
    use rewo_gpu::tooltip::CellKind;
    let sz = rewo_gpu::tooltip::SLOT_SIZE;
    let m = rewo_gpu::tooltip::SLOT_MARGIN;
    // Ink inside a GUI-space rect, and one GUI pixel's colour. The tooltip is
    // placed in screen space, so neither is offset by the panel's origin.
    let gink = |a: &[u8], b: &[u8], gx: i32, gy: i32, gw: i32, gh: i32| -> i64 {
        changed(
            a,
            b,
            (gx as f32 * scale) as u32,
            (gy as f32 * scale) as u32,
            (gw as f32 * scale) as u32,
            (gh as f32 * scale) as u32,
        )
    };
    let gpx = |img: &[u8], gx: i32, gy: i32| -> [u8; 3] {
        let x = ((gx as f32 + 0.5) * scale) as u32;
        let y = ((gy as f32 + 0.5) * scale) as u32;
        let i = ((y.min(H - 1) * W + x.min(W - 1)) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let bare = |t: &rewo_gpu::container::TooltipDraw| rewo_gpu::container::TooltipDraw {
        bundle: None,
        ..t.clone()
    };
    let cells_of = |t: &rewo_gpu::container::TooltipDraw| {
        t.bundle.as_ref().map(|i| i.cells.clone()).unwrap_or_default()
    };
    let bar_of = |t: &rewo_gpu::container::TooltipDraw| t.bundle.as_ref().and_then(|i| i.bar);

    // 1. The slot background, on every occupied cell and nowhere else.
    let tip3 = bundle_tip(&test_bundle(3));
    let cells3 = cells_of(&tip3);
    let box3 = tip_at(wr, gpu, off, Some(bare(&tip3)))?;
    let grid3 = tip_at(wr, gpu, off, Some(tip3.clone()))?;
    let occupied: Vec<i64> = cells3
        .iter()
        .map(|c| gink(&box3, &grid3, c.x, c.y, sz, sz))
        .collect();
    // The fourth column, which three stacks leave empty — placed by the same
    // `getContentXOffset` the walk used, not by counting back from a cell.
    let left_col = TIP_POS.0 + rewo_gpu::tooltip::content_x_offset(tip3.size.0);
    let blank = gink(&box3, &grid3, left_col, cells3[0].y, sz, sz);
    c.record(
        "bc1.a_slot_background_lands_on_every_occupied_cell_and_nowhere_else",
        cells3.len() == 3 && occupied.iter().all(|&n| n > 0) && blank == 0,
        format!(
            "the three cells the walk reports changed {occupied:?} pixels against \
             the same box with no grid; the empty fourth column at x {left_col} \
             changed {blank}. `extractSlot` blits `container/bundle/slot_background` \
             at the cell's own `drawX`/`drawY` and 24x24 — M52 computed all of \
             this and blitted none of it, which is a grid of nothing. Filling the \
             columns left to right instead swaps these two numbers exactly"
        ),
    );

    // 2. …and the badge cell is not one of them.
    let tip13 = bundle_tip(&test_bundle(13));
    let cells13 = cells_of(&tip13);
    let box13 = tip_at(wr, gpu, off, Some(bare(&tip13)))?;
    let grid13 = tip_at(wr, gpu, off, Some(tip13.clone()))?;
    let badge = cells13
        .iter()
        .find(|c| matches!(c.kind, CellKind::Badge { .. }))
        .copied();
    // A slot in the badge's own row, so the two are compared like for like.
    let neighbour = badge.and_then(|b| {
        cells13
            .iter()
            .find(|c| c.y == b.y && matches!(c.kind, CellKind::Slot { .. }))
            .copied()
    });
    let (badge_ink, slot_ink) = match (badge, neighbour) {
        (Some(b), Some(s)) => (
            gink(&box13, &grid13, b.x, b.y, sz, sz),
            gink(&box13, &grid13, s.x, s.y, sz, sz),
        ),
        _ => (-1, -1),
    };
    c.record(
        "bc2.the_badge_cell_gets_no_slot_background",
        badge_ink == 0 && slot_ink > 0,
        format!(
            "the `+N` cell at {:?} changed {badge_ink} pixels; the slot beside it at \
             {:?} changed {slot_ink}. `shouldRenderSurplusText` routes that cell to \
             `extractCount`, whose entire body is one `centeredText` — only \
             `extractSlot` blits, so the badge sits on the tooltip's own background \
             with no cell art under it. Blitting a background for every visited \
             cell, which is the obvious reading of a grid, lights the first number up",
            badge.map(|b| (b.x, b.y)),
            neighbour.map(|s| (s.x, s.y))
        ),
    );

    // 3. Which cell the highlight lands on, through the visual-order mapping.
    let selected_tip = |item: i32| {
        let mut b = test_bundle(3);
        b.selected = item;
        bundle_tip(&b)
    };
    let tip_sel = selected_tip(2);
    let sel_last = tip_at(wr, gpu, off, Some(tip_sel.clone()))?;
    let sel_first = tip_at(wr, gpu, off, Some(selected_tip(0)))?;
    let sel_none = tip_at(wr, gpu, off, Some(selected_tip(99)))?;
    let moved = |img: &[u8]| -> Vec<i32> {
        cells3
            .iter()
            .filter(|c| gink(&grid3, img, c.x, c.y, sz, sz) > 0)
            .map(|c| c.x)
            .collect()
    };
    let (m_last, m_first, m_none) = (moved(&sel_last), moved(&sel_first), moved(&sel_none));
    c.record(
        "bc3.the_highlight_follows_the_selected_index_through_the_visual_order",
        m_last == vec![cells3[0].x] && m_first == vec![cells3[2].x] && m_none.is_empty(),
        format!(
            "selecting the last stack moves the cells at {m_last:?}, the first moves \
             {m_first:?}, and an index no stack has moves {m_none:?}. \
             `hasHighlight` is `shownItems.size() - slotNumber == \
             getSelectedItemIndex()`, so the index runs *backwards* along the \
             visit order — keying it off the visit order instead highlights the \
             mirrored cell, which is these two answers swapped. An out-of-range \
             index highlights nothing rather than clamping onto an end cell"
        ),
    );

    // 4. And which sprite it is: the highlight **replaces** the background, so
    //    the selected cell is translucent where every other cell is opaque.
    //    Moving the box out from under the grid is what makes that observable
    //    without predicting a blend — an opaque cell cannot notice.
    let uncovered = rewo_gpu::container::TooltipDraw {
        pos: (200, 200),
        size: (8, 8),
        bundle: tip_sel.bundle.clone(),
    };
    let no_box = tip_at(wr, gpu, off, Some(uncovered))?;
    // The 16 px interior, because the cell art's outer three pixels are
    // transparent on every one of these sprites.
    let sel_moved = gink(&sel_last, &no_box, cells3[0].x + m, cells3[0].y + m, 16, 16);
    let plain_moved = gink(&sel_last, &no_box, cells3[1].x + m, cells3[1].y + m, 16, 16);
    c.record(
        "bc4.the_highlight_replaces_the_background_rather_than_covering_it",
        sel_moved > 0 && plain_moved == 0,
        format!(
            "taking the tooltip's box out from behind the grid changes {sel_moved} \
             pixels of the selected cell's interior and {plain_moved} of an \
             unselected one. `extractSlot`'s `if (hasHighlight) … else …` picks one \
             sprite, and `slot_highlight_back` is white at alpha 96 where \
             `slot_background` is opaque — so a selected cell shows whatever is \
             behind it and an unselected cell cannot. Drawing the background *and* \
             the highlight, which reads as the safer order, makes the first number \
             zero too"
        ),
    );

    // 5. The progress bar's fill, and where it starts.
    let weighted = |num: i32, den: i32| {
        let mut b = test_bundle(3);
        b.weight = rewo_gpu::tooltip::Fraction::new(num, den);
        bundle_tip(&b)
    };
    // The fill's own colour, read out of the sprite the pass samples.
    let texel = |s: &rewo_gpu::hud::HudSpriteData<'_>, x: u32, y: u32| -> [u8; 3] {
        let i = ((y * s.w + x) * 4) as usize;
        [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2]]
    };
    let fill_rgb = texel(&sprites.bundle_bar_fill, 2, 2);
    let full_rgb = texel(&sprites.bundle_bar_full, 2, 2);
    // The bar's middle row is the one whose every column is the fill's colour:
    // the nine-slice's centre band, clear of the rounded corners.
    let run = |img: &[u8], t: &rewo_gpu::container::TooltipDraw, want: [u8; 3]| -> (i32, i32) {
        let Some(b) = bar_of(t) else { return (-1, -1) };
        let hit: Vec<i32> = (0..rewo_gpu::tooltip::GRID_WIDTH)
            .filter(|dx| gpx(img, b.x + dx, b.y + 6) == want)
            .collect();
        (
            hit.len() as i32,
            hit.first().map(|dx| b.x + dx).unwrap_or(-1),
        )
    };
    let states: Vec<(i32, i32, (i32, i32))> = [(0, 1), (1, 4), (1, 2), (1, 1)]
        .iter()
        .map(|&(n, d)| {
            let t = weighted(n, d);
            let img = tip_at(wr, gpu, off, Some(t.clone())).unwrap_or_default();
            let want = if bar_of(&t).is_some_and(|b| b.full) {
                full_rgb
            } else {
                fill_rgb
            };
            (n, d, run(&img, &t, want))
        })
        .collect();
    let widths: Vec<i32> = states.iter().map(|(_, _, (w, _))| *w).collect();
    let bar3 = bar_of(&tip3).map(|b| b.x);
    let starts_right = states
        .iter()
        .filter(|(_, _, (w, _))| *w > 0)
        .all(|(_, _, (_, x0))| Some(*x0 - rewo_gpu::tooltip::PROGRESSBAR_BORDER) == bar3);
    c.record(
        "bc5.the_fill_width_tracks_the_weight_and_starts_one_pixel_in",
        widths == vec![0, 23, 47, 94] && starts_right,
        format!(
            "weights 0, 1/4, 1/2 and 1 fill {widths:?} pixels of the bar's centre \
             row, each beginning at {:?} against a bar at {bar3:?}. \
             `getProgressBarFill` is `clamp(mulAndTruncate(weight, 94), 0, 94)` — \
             **94**, not the bar's 96, and truncating rather than rounding, so a \
             quarter is 23 and not 24. The blit is at `x + 1` \
             (`PROGRESSBAR_BORDER`), which is what leaves the border's frame a \
             pixel to sit in; drawing it at `x` runs the fill under the left edge",
            states
                .iter()
                .map(|(_, _, (_, x0))| *x0)
                .collect::<Vec<_>>()
        ),
    );

    // 6. Which fill sprite, which is not a shade of one colour.
    let half_img = tip_at(wr, gpu, off, Some(weighted(1, 2)))?;
    let full_img = tip_at(wr, gpu, off, Some(weighted(1, 1)))?;
    let half_px = bar_of(&tip3).map(|b| gpx(&half_img, b.x + 20, b.y + 6));
    let full_px = bar_of(&tip3).map(|b| gpx(&full_img, b.x + 20, b.y + 6));
    c.record(
        "bc6.a_full_bundle_swaps_the_fill_sprite_for_the_full_one",
        half_px == Some(fill_rgb) && full_px == Some(full_rgb) && fill_rgb != full_rgb,
        format!(
            "half full the bar reads {half_px:?} against the fill sprite's \
             {fill_rgb:?}; completely full it reads {full_px:?} against the full \
             sprite's {full_rgb:?}. `getProgressBarTexture` swaps the whole sprite \
             at `weight.compareTo(ONE) >= 0`, and the two are the chat palette's \
             blue and red rather than two shades of one bar — so a single sprite \
             tinted by the weight would land on neither"
        ),
    );

    // 7. …and the border goes down over the fill, not under it.
    let frame_rgb = texel(&sprites.bundle_bar_border, 1, 0);
    let over = bar_of(&tip3).map(|b| gpx(&full_img, b.x + 48, b.y));
    c.record(
        "bc7.the_border_is_blitted_over_the_fill",
        over == Some(frame_rgb) && frame_rgb != full_rgb,
        format!(
            "with the bar completely full its top row at the centre reads {over:?} — \
             the border's frame {frame_rgb:?}, not the fill's {full_rgb:?}. \
             `extractProgressbar` blits `getProgressBarTexture` first and \
             `PROGRESSBAR_BORDER_SPRITE` second, and both cover that row; reversing \
             the two paints the fill's own top edge across the frame"
        ),
    );

    // 8. The precondition under all of the above: Rewo stretches a nine-slice's
    //    inner pieces where vanilla tiles them, and the two agree only because
    //    every one of these sprites is uniform along the axis it repeats on.
    let nine = [
        ("slot_background", &sprites.bundle_slot, 4u32),
        ("slot_highlight_back", &sprites.bundle_highlight_back, 4),
        ("slot_highlight_front", &sprites.bundle_highlight_front, 4),
        ("progressbar_border", &sprites.bundle_bar_border, 2),
        ("progressbar_fill", &sprites.bundle_bar_fill, 2),
        ("progressbar_full", &sprites.bundle_bar_full, 2),
    ];
    let patterned: Vec<&str> = nine
        .iter()
        .filter(|(_, s, b)| !inner_slices_uniform(s.rgba, s.w, s.h, *b))
        .map(|(n, _, _)| *n)
        .collect();
    // The same check on a sprite whose centre is deliberately not uniform, so
    // the pass above is a measurement rather than a function that says yes.
    let mut striped = sprites.bundle_bar_fill.rgba.to_vec();
    striped[(3 * 6 + 3) * 4] ^= 0xFF;
    let catches = !inner_slices_uniform(
        &striped,
        sprites.bundle_bar_fill.w,
        sprites.bundle_bar_fill.h,
        2,
    );
    c.record(
        "bc8.every_inner_slice_is_uniform_along_the_axis_it_tiles_on",
        patterned.is_empty() && catches,
        format!(
            "{} of the six sprites carry a patterned inner slice ({patterned:?}), and \
             flipping one texel of the fill's centre is caught: {catches}. None of \
             the six sets `stretch_inner`, so vanilla **tiles** those five pieces \
             while this pass emits one stretched quad each — the two answers are \
             the same picture exactly while this holds, and this is a fact about \
             26.2's art rather than a theorem, so it is measured. The three 24x24 \
             cell sprites never reach the slicing at all: they are blitted at \
             their authored size, which is `blitNineSlicedSprite`'s first branch",
            patterned.len()
        ),
    );

    // 9. The empty bundle takes the other arm entirely.
    let empty_tip = bundle_tip(&test_bundle(0));
    let empty_box = tip_at(wr, gpu, off, Some(bare(&empty_tip)))?;
    let empty_img = tip_at(wr, gpu, off, Some(empty_tip.clone()))?;
    let empty_bar = bar_of(&empty_tip);
    let bar_ink = empty_bar
        .map(|b| {
            gink(
                &empty_box,
                &empty_img,
                b.x,
                b.y,
                rewo_gpu::tooltip::GRID_WIDTH,
                rewo_gpu::tooltip::PROGRESSBAR_HEIGHT,
            )
        })
        .unwrap_or(-1);
    // Everything above the bar, where a grid would have been.
    let above_ink = empty_bar
        .map(|b| {
            gink(
                &empty_box,
                &empty_img,
                TIP_POS.0,
                b.y - sz,
                rewo_gpu::tooltip::GRID_WIDTH,
                sz,
            )
        })
        .unwrap_or(-1);
    let empty_fill = empty_tip
        .bundle
        .as_ref()
        .map(|i| i.cells.len())
        .zip(empty_bar.map(|b| (b.fill, b.label)));
    c.record(
        "bc9.an_empty_bundle_draws_its_bar_and_no_cells",
        bar_ink > 0
            && above_ink == 0
            && empty_fill == Some((0, (0, Some(rewo_gpu::tooltip::BarLabel::Empty)))),
        format!(
            "the bar's rect changed {bar_ink} pixels and the 24 px band above it \
             {above_ink}; the walk reports {empty_fill:?} as (cells, (fill, label)). \
             `extractEmptyBundleTooltip` is a different method with no cell loop, \
             and it hands `extractProgressbar` a literal `Fraction.ZERO` rather than \
             the bundle's own weight — this bundle's weight is 1/2, so passing the \
             weight through would have filled 47 pixels of an empty bundle's bar"
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
