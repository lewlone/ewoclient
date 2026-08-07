//! `rewo containershot --check` — the container-screen gate (M87).
//!
//! Serverless, validation-required, fail-closed. It grades the thing eleven
//! commits of decode and geometry were for: that opening a chest paints *that
//! chest's* sheet, at *its* size, with *its* slots.
//!
//! # What it grades against
//!
//! Not the layout table. `menu_layout` and `menu_screen` are hand transcriptions,
//! and a gate that imports them asserts only that they equal themselves — M41's
//! `t4` and M59's recorded failure mode. So the slot geometry is re-derived here
//! from `ChestMenu`'s constructor independently (`addChestGrid(container, 8, 18)`
//! then `addStandardInventorySlots(inventory, 8, 18 + rows * 18 + 13)`), and the
//! panel is graded against **`generic_54.png` itself**, read straight out of the
//! jar — which is an oracle neither table can influence.
//!
//! # The witness that matters
//!
//! A chest's background is two blits out of one sheet: the top `rows * 18 + 17`
//! px from `v = 0`, then 96 px from `v = 126`. Everything between is rows the
//! panel skips. So the gate probes **both bands**, and the lower band's probes
//! are what catch a wrong `v` — a single stretched blit, or a second band taken
//! from the wrong row, still paints a plausible chest and differs only where the
//! sheet's own content differs.

use clap::Args as ClapArgs;

use crate::stats::OverlayRing;
use rewo_data::assets;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

const W: u32 = 1280;
const H: u32 = 720;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

#[derive(ClapArgs, Debug)]
pub struct ContainershotArgs {
    /// Fail-closed grading. Without it the command renders and reports only.
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    /// Dump the rendered frames as an eyeball artifact.
    #[arg(long)]
    pub out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn new() -> Self {
        Self {
            witnessed: 0,
            failures: Vec::new(),
        }
    }

    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        let status = if pass { " ok " } else { "FAIL" };
        println!("[containershot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// ---------------------------------------------------------------------------
// An independent re-derivation of ChestMenu's slot list.
//
// Transcribed here from the decompiled constructor rather than read out of
// `menu_layout`, so the two can disagree and the gate can say so.
// ---------------------------------------------------------------------------

fn chest_slots_independently(rows: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    // addChestGrid(container, 8, 18)
    for y in 0..rows {
        for x in 0..9 {
            out.push((8 + x * 18, 18 + y * 18));
        }
    }
    // addStandardInventorySlots(inventory, 8, 18 + containerRows * 18 + 13)
    let top = 18 + rows * 18 + 13;
    for y in 0..3 {
        for x in 0..9 {
            out.push((8 + x * 18, top + y * 18));
        }
    }
    // ...whose hotbar is at `top + topToHotbar`, and topToHotbar is 58.
    for x in 0..9 {
        out.push((8 + x * 18, top + 58));
    }
    out
}

pub fn run(args: ContainershotArgs) -> Result<(), String> {
    let paths = rewo_data::DataPaths::for_version(&args.version)
        .ok_or("no config dir for version data")?;
    let jar = crate::inventoryshot_cmd::client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut c = Checker::new();

    // -- the model half, graded against the independent re-derivation --------

    for rows in 1..=6i32 {
        let id = rows - 1;
        let layout = rewo_world::menu_layout::layout_of(id)
            .ok_or_else(|| format!("containershot: no layout for menu {id}"))?;
        let mine = chest_slots_independently(rows);
        let theirs: Vec<(i32, i32)> = layout
            .positions()
            .into_iter()
            .map(|(x, y)| (x as i32, y as i32))
            .collect();
        c.record(
            &format!("m{rows}.generic_9x{rows}_slots_match_an_independent_derivation"),
            mine == theirs,
            format!(
                "{} slots; first difference {:?}",
                theirs.len(),
                mine.iter()
                    .zip(&theirs)
                    .position(|(a, b)| a != b)
                    .map(|i| (i, mine.get(i), theirs.get(i)))
            ),
        );
    }

    // The player's menu and the lectern must resolve to no container panel,
    // for two different reasons; both are load-bearing.
    c.record(
        "m7.the_players_own_menu_is_not_a_container_panel",
        crate::live_cmd::container_panel_for_test(&rewo_world::menu_layout::PLAYER).is_none(),
        "PLAYER carries NO_PROTOCOL_ID and draws from the pass's inventory.png rect",
    );
    c.record(
        "m8.the_lectern_paints_no_panel",
        rewo_world::menu_layout::layout_of(17)
            .and_then(crate::live_cmd::container_panel_for_test)
            .is_none(),
        "LecternScreen is a BookViewScreen — a fallthrough would paint another menu's sheet",
    );

    // -- M89: the shown menu is the one every consumer uses -----------------
    //
    // M87 rendered the container while every click still operated on the
    // player's inventory, so clicking a chest's slot 5 picked up the player's
    // crafting grid, and the click packet named container 0 with the player's
    // state id. These grade the accessor all of them now go through.

    let mut session_menus = rewo_world::menu::Menus::new();
    let player = rewo_world::inventory::Inventory::default();
    c.record(
        "s1.with_nothing_open_the_shown_menu_is_the_players",
        session_menus.open().is_none(),
        "no container -> Inventory::default(), container id 0",
    );
    session_menus.apply_open_screen(7, 2, "Chest".into());
    let shown = session_menus.open().map(|m| &m.menu).unwrap_or(&player);
    c.record(
        "s2.with_a_container_open_the_shown_menu_is_the_container",
        shown.slot_count() == 63 && shown.layout().name == "generic_9x3",
        format!(
            "{} slots, layout {} (the player's is 46)",
            shown.slot_count(),
            shown.layout().name
        ),
    );
    // The hover geometry the click path uses. A chest's slot 0 sits at (8, 18)
    // in its own panel; the player's slot 0 is the crafting RESULT at
    // (154, 28). Asking the player's layout while a chest is up therefore does
    // not merely shift the answer — it indexes a different kind of slot.
    let chest = rewo_world::menu_layout::layout_of(2).unwrap();
    c.record(
        "s3.a_chests_slot_zero_is_its_grid_not_the_players_result",
        chest.position(0) == Some((8, 18))
            && rewo_world::menu_layout::PLAYER.position(0) == Some((154, 28)),
        format!(
            "chest slot 0 {:?}, player slot 0 {:?}",
            chest.position(0),
            rewo_world::menu_layout::PLAYER.position(0)
        ),
    );
    // And the two panels centre differently, so the same cursor position maps
    // to different GUI coordinates in each.
    let (_, chest_top, _) = rewo_gpu::container::gui_origin_for(
        W as f32,
        H as f32,
        chest.image_w as f32,
        chest.image_h as f32,
    );
    let (_, player_top, _) = rewo_gpu::container::gui_origin(W as f32, H as f32);
    c.record(
        "s4.the_two_panels_do_not_share_an_origin",
        chest_top != player_top,
        format!("chest top {chest_top}, player top {player_top} — a hover asked of the wrong one is off by their difference"),
    );

    // -- M93: the derivation, not a constructed copy of it -------------------
    //
    // Every unit test of the single-input quick-moves hand-builds an
    // `ItemProps`, so none of them can see whether the LIVE client resolves
    // `beacon_payment` at all. That is M92's finding restated: when a gate
    // supplies an input production must derive, the derivation is untested by
    // construction. So this calls the production `live_cmd::item_props` and
    // grades what it actually returns for real registry ids.
    {
        let items = rewo_data::items::Items::load(&paths.registries_json())?;
        let id_of = |name: &str| -> Result<i32, String> {
            (0..)
                .take(4096)
                .find(|&i| items.name(i) == Some(name))
                .ok_or_else(|| format!("containershot: {name} not in the item registry"))
        };
        let iron = id_of("minecraft:iron_ingot")?;
        let stick = id_of("minecraft:stick")?;
        let andesite = id_of("minecraft:andesite")?;
        let iron_p = crate::live_cmd::item_props(&items, iron)
            .ok_or("containershot: item_props declined a real id")?;
        let stick_p = crate::live_cmd::item_props(&items, stick)
            .ok_or("containershot: item_props declined a real id")?;
        let andesite_p = crate::live_cmd::item_props(&items, andesite)
            .ok_or("containershot: item_props declined a real id")?;
        c.record(
            "d1.the_live_client_resolves_the_beacon_payment_tag",
            iron_p.beacon_payment && !stick_p.beacon_payment,
            format!(
                "production item_props: iron_ingot(id {iron}) payment={}, stick(id {stick}) payment={} \
                 — graded through the same function the live click uses, not a copy",
                iron_p.beacon_payment, stick_p.beacon_payment
            ),
        );
        // The negative half on its own would pass against a function that
        // resolved NOTHING, so pin something the same call must also get right.
        c.record(
            "d2.the_same_call_still_resolves_the_furnace_predicates",
            stick_p.is_fuel && !iron_p.is_fuel && iron_p.smeltable == [false; 3],
            format!(
                "stick is_fuel={} iron is_fuel={} iron smeltable={:?} — so m1's \
                 `false` is a real answer and not a dead lookup",
                stick_p.is_fuel, iron_p.is_fuel, iron_p.smeltable
            ),
        );
        // M93b, built in with the feature rather than after it. Same reason:
        // every stonecutter unit test hand-builds its ItemProps, so without
        // this the live lookup could be wired to nothing.
        c.record(
            "d3.the_live_client_resolves_the_stonecutter_input_set",
            andesite_p.stonecuttable && !stick_p.stonecuttable && !iron_p.stonecuttable,
            format!(
                "production item_props: andesite(id {andesite}) cuttable={}, stick cuttable={}, \
                 iron_ingot cuttable={}",
                andesite_p.stonecuttable, stick_p.stonecuttable, iron_p.stonecuttable
            ),
        );
        // M93e — the grindstone's PROTOTYPE half. `isDamageableItem` needs
        // `has(MAX_DAMAGE) && has(DAMAGE)`, and for every vanilla item both
        // come from the prototype, so if this resolver returned false the
        // grindstone would refuse every tool while all six unit witnesses —
        // which hand-build the props — stayed green.
        let sword = id_of("minecraft:diamond_sword")?;
        let sword_p = crate::live_cmd::item_props(&items, sword)
            .ok_or("containershot: item_props declined a real id")?;
        c.record(
            "d5.the_live_client_resolves_the_damageable_prototype_components",
            sword_p.proto_max_damage
                && sword_p.proto_damage
                && !stick_p.proto_max_damage
                && !stick_p.proto_damage,
            format!(
                "diamond_sword(id {sword}) max_damage={} damage={}; stick max_damage={} damage={}",
                sword_p.proto_max_damage,
                sword_p.proto_damage,
                stick_p.proto_max_damage,
                stick_p.proto_damage
            ),
        );
        // M93f — the cartography table's additional-slot set is three item
        // identities, resolved here rather than from a table. If this returned
        // false for all three, paper would cross-move to the hotbar while
        // every unit witness — which hand-builds the props — stayed green.
        let paper = id_of("minecraft:paper")?;
        let map = id_of("minecraft:map")?;
        let pane = id_of("minecraft:glass_pane")?;
        let filled = id_of("minecraft:filled_map")?;
        let props_of = |id: i32| crate::live_cmd::item_props(&items, id);
        let three = [paper, map, pane]
            .iter()
            .all(|&i| props_of(i).is_some_and(|p| p.cartography_additional));
        c.record(
            "d6.the_live_client_resolves_the_cartography_additional_items",
            three
                && !props_of(filled).is_some_and(|p| p.cartography_additional)
                && !stick_p.cartography_additional,
            format!(
                "paper/map/glass_pane all additional={three}; filled_map={}, stick={} \
                 — filled_map is a DIFFERENT item and routes by its MAP_ID component",
                props_of(filled).is_some_and(|p| p.cartography_additional),
                stick_p.cartography_additional
            ),
        );
        // M93g — the loom's three sets, through the production resolver.
        let banner = id_of("minecraft:white_banner")?;
        let dye = id_of("minecraft:red_dye")?;
        let pattern = id_of("minecraft:flower_banner_pattern")?;
        let shield = id_of("minecraft:shield")?;
        let b_p = props_of(banner).ok_or("containershot: item_props declined")?;
        let d_p = props_of(dye).ok_or("containershot: item_props declined")?;
        let pat_p = props_of(pattern).ok_or("containershot: item_props declined")?;
        let sh_p = props_of(shield).ok_or("containershot: item_props declined")?;
        c.record(
            "d7.the_live_client_resolves_the_looms_three_sets_disjointly",
            (b_p.loom_banner, b_p.loom_dye, b_p.loom_pattern) == (true, false, false)
                && (d_p.loom_banner, d_p.loom_dye, d_p.loom_pattern) == (false, true, false)
                && (pat_p.loom_banner, pat_p.loom_dye, pat_p.loom_pattern)
                    == (false, false, true),
            format!(
                "white_banner {:?}, red_dye {:?}, flower_banner_pattern {:?} (banner, dye, pattern)",
                (b_p.loom_banner, b_p.loom_dye, b_p.loom_pattern),
                (d_p.loom_banner, d_p.loom_dye, d_p.loom_pattern),
                (pat_p.loom_banner, pat_p.loom_dye, pat_p.loom_pattern)
            ),
        );
        // THE witness the tag exists for. A shield's prototype carries
        // `minecraft:banner_patterns` exactly as a banner's does, so a
        // component-derived banner set would put a shield in the loom's banner
        // slot — and every unit witness, which hand-builds its props, would
        // stay green while it did.
        c.record(
            "d8.a_shield_is_not_a_banner_though_it_carries_banner_patterns",
            !sh_p.loom_banner
                && rewo_data::item_components_table::prototype_has_component(
                    "minecraft:shield",
                    "minecraft:banner_patterns",
                ) == Some(true),
            format!(
                "shield loom_banner={}, but its prototype DOES carry banner_patterns ({:?}) \
                 — so the set came from #minecraft:banners and not from the component",
                sh_p.loom_banner,
                rewo_data::item_components_table::prototype_has_component(
                    "minecraft:shield",
                    "minecraft:banner_patterns",
                )
            ),
        );
        // d9 exists because a mutation SURVIVED and was shown to be equivalent
        // rather than fixed. Dropping `&& prototype_has(DYE)` from the
        // conjunction changes no answer the jar can produce, because every
        // item in `#minecraft:loom_dyes` also carries the component — which is
        // exactly the coincidence `loom_table`'s docs warn about. No fixture
        // built from vanilla data can distinguish the two readings.
        //
        // So this pins the COINCIDENCE instead of the behaviour: if a version
        // bump ever tags an item without the component, the conjunction starts
        // mattering and this fires to say so. (The *patch* half of the
        // conjunction IS reachable and is witnessed in rewo-world.)
        let tag_and_component_agree = |tag: &[&str], component: &str| {
            tag.iter().all(|i| {
                rewo_data::item_components_table::prototype_has_component(i, component)
                    == Some(true)
            })
        };
        let dyes_agree = tag_and_component_agree(
            rewo_data::loom_table::LOOM_DYES,
            "minecraft:dye",
        );
        let patterns_agree = tag_and_component_agree(
            rewo_data::loom_table::LOOM_PATTERNS,
            "minecraft:provides_banner_patterns",
        );
        c.record(
            "d9.the_looms_tags_and_components_still_coincide",
            dyes_agree && patterns_agree,
            format!(
                "every one of {} loom dyes carries minecraft:dye ({dyes_agree}) and every one \
                 of {} loom patterns carries provides_banner_patterns ({patterns_agree}) — \
                 while that holds, the conjunction's second term is unobservable on vanilla \
                 data and is kept because vanilla writes it",
                rewo_data::loom_table::LOOM_DYES.len(),
                rewo_data::loom_table::LOOM_PATTERNS.len()
            ),
        );
        // M93h — the crafter's toggle packet must RESOLVE, not merely exist as
        // a field. `container_slot_state_changed` is a different packet from
        // `container_button_click`, and a name that does not resolve leaves
        // the sender returning Err forever while every unit witness — which
        // grades the body builder, not the id — stays green.
        {
            let p = rewo_data::packets::Packets::load(&paths.packets_json())?;
            let ids = rewo_net::ids::Ids::resolve(&p)?;
            c.record(
                "d10.the_crafters_toggle_packet_resolves_and_is_not_the_button_click",
                ids.sb_play_container_slot_state_changed.is_some()
                    && ids.sb_play_container_slot_state_changed
                        != ids.sb_play_container_button_click,
                format!(
                    "container_slot_state_changed={:?}, container_button_click={:?} — two \
                     different serverbound packets, and CrafterMenu has no clickMenuButton \
                     override at all",
                    ids.sb_play_container_slot_state_changed,
                    ids.sb_play_container_button_click
                ),
            );
            // M93l — the beacon's confirm packet, same rule: the body-builder
            // witness grades bytes, not the id, so a name that never resolved
            // would leave the sender returning Err forever while every unit
            // test stayed green.
            c.record(
                "d11.the_beacons_confirm_packet_resolves_and_is_its_own",
                ids.sb_play_set_beacon.is_some()
                    && ids.sb_play_set_beacon != ids.sb_play_container_button_click
                    && ids.sb_play_set_beacon != ids.sb_play_container_slot_state_changed,
                format!(
                    "set_beacon={:?} — distinct from container_button_click {:?} and                      container_slot_state_changed {:?}; the beacon's confirm is neither                      of those, despite its chrome being ordinary buttons",
                    ids.sb_play_set_beacon,
                    ids.sb_play_container_button_click,
                    ids.sb_play_container_slot_state_changed
                ),
            );
            // M93n — the anvil's rename, the fourth distinct serverbound
            // packet the container arc has needed. Four screens, four packets,
            // and none of them a mode of another.
            let four = [
                ids.sb_play_container_button_click,
                ids.sb_play_container_slot_state_changed,
                ids.sb_play_set_beacon,
                ids.sb_play_rename_item,
            ];
            let all_resolved = four.iter().all(|i| i.is_some());
            let all_distinct = {
                let mut v: Vec<i32> = four.iter().flatten().copied().collect();
                v.sort_unstable();
                let n = v.len();
                v.dedup();
                v.len() == n
            };
            c.record(
                "d12.the_anvils_rename_resolves_and_the_four_screen_packets_are_distinct",
                all_resolved && all_distinct,
                format!(
                    "button_click/slot_state/set_beacon/rename_item = {four:?} — four                      screens, four packets, and a name that failed to resolve would                      leave its sender returning Err while every body witness stayed green"
                ),
            );
        }
        // ...and the two jar-derived recipe tables must not be the same data.
        // Stone is stonecuttable and NOT smeltable; iron ore is the reverse.
        // Wiring both fields to one table would leave d3 green.
        let iron_ore = id_of("minecraft:iron_ore")?;
        let ore_p = crate::live_cmd::item_props(&items, iron_ore)
            .ok_or("containershot: item_props declined a real id")?;
        c.record(
            "d4.the_stonecutter_and_furnace_sets_are_different_data",
            andesite_p.stonecuttable
                && andesite_p.smeltable == [false; 3]
                && !ore_p.stonecuttable
                && ore_p.smeltable[1],
            format!(
                "andesite cuttable={} smeltable={:?}; iron_ore cuttable={} smeltable={:?}",
                andesite_p.stonecuttable,
                andesite_p.smeltable,
                ore_p.stonecuttable,
                ore_p.smeltable
            ),
        );
    }

    // -- the pixel half ------------------------------------------------------

    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[containershot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("containershot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    let r = pixels(&mut c, &mut gpu, &mut off, &mut wr, &baked, &args);
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    r?;

    println!(
        "[containershot] witnesses observed: {} / {}",
        c.witnessed,
        c.witnessed + c.failures.len()
    );
    if args.check && !c.failures.is_empty() {
        return Err(format!(
            "containershot: {} witness(es) failed: {}",
            c.failures.len(),
            c.failures.join(", ")
        ));
    }
    if args.check && c.witnessed == 0 {
        return Err("containershot: no witnesses ran".into());
    }
    println!("[containershot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

fn pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    baked: &assets::BakedAssets,
    args: &ContainershotArgs,
) -> Result<(), String> {
    let Some(sprites) = crate::live_cmd::container_sprites(baked) else {
        return Err("container sprites missing from the jar".into());
    };
    // `generic_54.png` straight out of the bake — the oracle the two tables
    // cannot influence.
    let sheet_i = crate::live_cmd::sheet_index_for_test("textures/gui/container/generic_54.png")
        .ok_or("containershot: generic_54 is not in the bake list")?;
    let sheet = &sprites.menu_backgrounds[sheet_i];
    let (sheet_w, sheet_rgba) = (sheet.w, sheet.rgba.to_vec());
    wr.init_container(gpu, &sprites)?;

    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let mut shot = |gpu: &mut Gpu, off: &mut Offscreen, wr: &mut WorldRenderer| {
        off.render(gpu, Some((&mut *wr, vp)), &draw, CLEAR)?;
        off.read_rgba(gpu)
    };

    wr.set_gui_items(gpu, &[])?;

    // A ONE-row chest for the band probes. The six-row chest was the obvious
    // choice and is the wrong one, which `p3` caught on this gate's first run:
    // at six rows `split` is 125, so the lower band maps `y -> y + 1` and the
    // two readings land on ADJACENT sheet rows, which are identical wherever
    // the art is flat. `p2` then passes without discriminating anything. At
    // one row the offset is `126 - 35 = 91`, which is most of the sheet.
    let rows = 1i32;
    let chest = rewo_world::menu_layout::layout_of(rows - 1).unwrap();
    wr.set_container_panel(crate::live_cmd::container_panel_for_test(chest));
    wr.set_container(true, None);
    let open = shot(gpu, off, wr)?;
    if let Some(d) = &args.out_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("out-dir: {e}"))?;
        let _ = off.save_png(gpu, &d.join("containershot-chest.png"));
    }

    let (left, top, scale) = rewo_gpu::container::gui_origin_for(
        W as f32,
        H as f32,
        chest.image_w as f32,
        chest.image_h as f32,
    );
    let at = |img: &[u8], gx: i32, gy: i32| -> [u8; 3] {
        let x = (left + (gx as f32 + 0.5) * scale) as u32;
        let y = (top + (gy as f32 + 0.5) * scale) as u32;
        let i = ((y.min(H - 1) * W + x.min(W - 1)) * 4) as usize;
        [img[i], img[i + 1], img[i + 2]]
    };
    let src = |sx: i32, sy: i32| -> [u8; 3] {
        let i = ((sy as u32 * sheet_w + sx as u32) * 4) as usize;
        [sheet_rgba[i], sheet_rgba[i + 1], sheet_rgba[i + 2]]
    };

    // The band boundary: the top blit ends here, the second starts at v = 126.
    let split = rows * 18 + 17;

    // Probes in the TOP band map straight through (dst y == src v).
    let top_probes = [(20, 30), (90, 60), (150, 100), (8, 120)];
    let top_wrong: Vec<_> = top_probes
        .iter()
        .filter(|&&(x, y)| y < split && at(&open, x, y) != src(x, y))
        .collect();
    c.record(
        "p1.the_top_band_reproduces_generic_54_byte_for_byte",
        top_wrong.is_empty(),
        format!(
            "{}/{} probes match the sheet at the same row",
            top_probes.len() - top_wrong.len(),
            top_probes.len()
        ),
    );

    // Probes in the LOWER band are the ones that catch a wrong `v`: dst y maps
    // to src `126 + (y - split)`, not to y. A single stretched blit, or a
    // second band taken from the wrong row, still paints a plausible chest.
    let low_probes = [(20, split + 10), (90, split + 40), (150, split + 80)];
    let low_wrong: Vec<_> = low_probes
        .iter()
        .filter(|&&(x, y)| at(&open, x, y) != src(x, 126 + (y - split)))
        .collect();
    c.record(
        "p2.the_lower_band_comes_from_v_126_not_from_its_own_row",
        low_wrong.is_empty(),
        format!(
            "{}/{} probes match the sheet at 126 + (y - {split})",
            low_probes.len() - low_wrong.len(),
            low_probes.len()
        ),
    );

    // ...and the mutation that would pass p1 and fail p2: the same probes read
    // against their own row rather than the second band. If the sheet happens
    // to be identical there this witness is vacuous, so it says which.
    let naive_same = low_probes
        .iter()
        .all(|&(x, y)| src(x, y) == src(x, 126 + (y - split)));
    c.record(
        "p3.the_lower_band_probes_can_tell_the_two_readings_apart",
        !naive_same,
        "the sheet's own rows differ from its v=126 band at these probes, so p2 is not vacuous",
    );

    // Centring is measured on the SIX-row chest, not the one-row one above:
    // a 132-tall panel differs from the player's 166 by 17 px, while a
    // 222-tall one differs by 28 — and the point is that the difference is
    // real and in the right direction, which a 2 px gap would not show.
    let tall = rewo_world::menu_layout::layout_of(5).unwrap();
    let (_, tall_top, _) = rewo_gpu::container::gui_origin_for(
        W as f32,
        H as f32,
        tall.image_w as f32,
        tall.image_h as f32,
    );
    let (_, player_top, _) = rewo_gpu::container::gui_origin(W as f32, H as f32);
    let expected = player_top - ((tall.image_h - 166) as f32 / 2.0 * scale).floor();
    c.record(
        "p4.the_panel_is_centred_for_its_own_height",
        (tall_top - expected).abs() <= scale && tall_top < player_top,
        format!(
            "six-row chest top {tall_top}, player top {player_top}, expected about {expected}"
        ),
    );

    // With no panel set, the pass draws the player's inventory exactly as it
    // did before M87 — which is what holds `inventoryshot` still.
    wr.set_container_panel(None);
    let player = shot(gpu, off, wr)?;
    c.record(
        "p5.clearing_the_panel_returns_to_the_players_inventory",
        player != open,
        "the two frames differ, so the panel really is what selected the sheet",
    );

    overlays(c, gpu, off, wr, baked, args, &mut shot)?;

    Ok(())
}

/// The M92 overlays: brewing, enchanting and the beacon, graded as **pixels**.
///
/// Every claim here is measured against a *control frame* of the same screen in
/// a different state, never against "non-black" or "differs from the clear
/// colour". That rule is the one this project keeps relearning — M34's
/// non-black-against-a-painted-sky, M38's three detectors in one milestone,
/// M50's `> 8` threshold — and the shape of the mistake is always the same: a
/// detector that answers about the background rather than about the subject.
fn overlays(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    baked: &assets::BakedAssets,
    args: &ContainershotArgs,
    shot: &mut impl FnMut(&mut Gpu, &mut Offscreen, &mut WorldRenderer) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    use rewo_world::menu::Menus;

    let mob_effects = rewo_data::mob_effects::MobEffects::load(
        &rewo_data::DataPaths::for_version("26.2")
            .ok_or("containershot: no data dir")?
            .registries_json(),
    )?;
    let effects = crate::live_cmd::beacon_effect_ids_for_test(&mob_effects);

    /// Open one menu with the given data slots set.
    fn menu(protocol_id: i32, data: &[(i16, i16)]) -> Menus {
        let mut m = Menus::new();
        assert!(m.apply_open_screen(1, protocol_id, "T".into()));
        for &(id, v) in data {
            assert!(m.apply_set_data(1, id, v), "slot {id}");
        }
        m
    }

    // GUI-pixel → screen-pixel, for a panel of the given size.
    let probe = |layout: &rewo_world::menu_layout::MenuLayout| {
        let (left, top, scale) = rewo_gpu::container::gui_origin_for(
            W as f32,
            H as f32,
            layout.image_w as f32,
            layout.image_h as f32,
        );
        move |img: &[u8], gx: i32, gy: i32| -> [u8; 3] {
            let x = (left + (gx as f32 + 0.5) * scale) as u32;
            let y = (top + (gy as f32 + 0.5) * scale) as u32;
            let i = ((y.min(H - 1) * W + x.min(W - 1)) * 4) as usize;
            [img[i], img[i + 1], img[i + 2]]
        }
    };

    // -- the brewing stand ---------------------------------------------------

    let brewing = rewo_world::menu_layout::layout_of(11).unwrap();
    let at_brew = probe(brewing);
    let mut frame = |m: &Menus,
                     gpu: &mut Gpu,
                     off: &mut Offscreen,
                     wr: &mut WorldRenderer|
     -> Result<Vec<u8>, String> {
        let open = m.open().expect("a menu must be open");
        wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
            open, 30, false, effects, None, None, None, None, None,
        ));
        wr.set_container(true, None);
        shot(gpu, off, wr)
    };

    // An idle stand is the control: fuelled, but not brewing. Both animations
    // live inside `if (tickCount > 0)`.
    let idle = frame(&menu(11, &[(0, 0), (1, 20)]), gpu, off, wr)?;

    // The bubbles' bottom edge, measured from the frames rather than computed:
    // the lowest row inside the bubble column that differs from the idle
    // control, at each of several tick values.
    let bubble_x = 68; // inside the 12px column at x = 63
    let bottoms: Vec<Option<i32>> = [2i16, 4, 6, 8, 10]
        .iter()
        .map(|&t| -> Result<Option<i32>, String> {
            let f = frame(&menu(11, &[(0, 300 + t), (1, 20)]), gpu, off, wr)?;
            Ok((0..60)
                .rev()
                .find(|&y| at_brew(&f, bubble_x, y) != at_brew(&idle, bubble_x, y)))
        })
        .collect::<Result<_, _>>()?;
    let drawn: Vec<i32> = bottoms.iter().flatten().copied().collect();
    c.record(
        "o1.the_bubbles_bottom_edge_is_pinned_across_frames",
        drawn.len() >= 3 && drawn.iter().all(|&b| b == drawn[0]),
        format!(
            "lowest changed row per frame: {bottoms:?} — the column grows upward from a fixed bottom"
        ),
    );

    // ...and the heights really do vary, or o1 would pass on a static sprite.
    let tops: Vec<Option<i32>> = [2i16, 6, 10]
        .iter()
        .map(|&t| -> Result<Option<i32>, String> {
            let f = frame(&menu(11, &[(0, 300 + t), (1, 20)]), gpu, off, wr)?;
            Ok((0..60).find(|&y| at_brew(&f, bubble_x, y) != at_brew(&idle, bubble_x, y)))
        })
        .collect::<Result<_, _>>()?;
    let distinct: std::collections::BTreeSet<_> = tops.iter().flatten().collect();
    c.record(
        "o2.and_the_top_edge_moves_so_o1_is_not_a_static_sprite",
        distinct.len() >= 2,
        format!("topmost changed row per frame: {tops:?}"),
    );

    // The blank frame, observed rather than assumed. `BUBBLELENGTHS` ends in
    // **0**, so one frame in seven paints no bubbles at all — the `None`s in
    // o1 and o2 above are that frame, and this turns them from an incidental
    // into a claim. At t = 306 the index is `306 / 2 % 7 = 6`.
    let blank = frame(&menu(11, &[(0, 306), (1, 20)]), gpu, off, wr)?;
    let blank_column = (0..60).all(|y| at_brew(&blank, bubble_x, y) == at_brew(&idle, bubble_x, y));
    c.record(
        "o2b.one_frame_in_seven_paints_no_bubbles_at_all",
        blank_column,
        "at 306 ticks (index 6, length 0) the bubble column is identical to an idle stand",
    );

    // An idle stand draws NEITHER animation: both are inside the tick guard.
    // Measured over the whole panel against a stand that is brewing.
    let brewing_frame = frame(&menu(11, &[(0, 200), (1, 20)]), gpu, off, wr)?;
    c.record(
        "o3.an_idle_stand_differs_from_a_brewing_one",
        idle != brewing_frame,
        "the tick guard is real — an idle stand paints no arrow and no bubbles",
    );

    // The fuel bar survives the tick guard, so an unfuelled idle stand and a
    // fuelled idle stand differ, and they differ *at the bar*.
    let unfuelled = frame(&menu(11, &[(0, 0), (1, 0)]), gpu, off, wr)?;
    let bar_differs = (60..78).any(|x| at_brew(&idle, x, 45) != at_brew(&unfuelled, x, 45));
    c.record(
        "o4.the_fuel_bar_is_outside_the_tick_guard",
        bar_differs,
        "a fuelled idle stand differs from an unfuelled one at y=45, x in 60..78",
    );

    // -- the enchanting table ------------------------------------------------

    let ench = rewo_world::menu_layout::layout_of(13).unwrap();
    let at_ench = probe(ench);
    let mut ench_frame = |costs: [i16; 3],
                          mouse: Option<(f64, f64)>,
                          creative: bool,
                          gpu: &mut Gpu,
                          off: &mut Offscreen,
                          wr: &mut WorldRenderer|
     -> Result<Vec<u8>, String> {
        let m = menu(13, &[(0, costs[0]), (1, costs[1]), (2, costs[2])]);
        let open = m.open().unwrap();
        // With no lapis in the slot, `creative` is the whole affordability
        // answer: false gives three UNAFFORDABLE rows, true three available
        // ones — same costs, different backgrounds.
        wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
            open, 30, creative, effects, mouse, None, None, None, None,
        ));
        wr.set_container(true, None);
        shot(gpu, off, wr)
    };

    // An empty row blits the disabled background and NO numeral; an
    // *unaffordable* one blits the SAME background plus its numeral. Comparing
    // those two is what isolates the numeral.
    //
    // The obvious control — empty against an affordable offer — was written
    // first and a mutation proved it vacuous: those two rows have different
    // BACKGROUNDS (disabled vs normal), and the background rect contains the
    // numeral rect, so suppressing every numeral left the probe passing. The
    // detector was measuring the background. Same shape as M34's non-black
    // against a painted sky and M50's threshold, and the fix is the same one:
    // hold everything but the subject constant.
    let empty = ench_frame([0, 0, 0], None, false, gpu, off, wr)?;
    let unaffordable = ench_frame([5, 10, 30], None, false, gpu, off, wr)?;
    // The WHOLE 16x16 numeral rect at (61, 15 + 19i), not one row of it: this
    // probed row `16 + 19i` first and failed, because that is the numeral's
    // second row and a glyph's top rows are transparent. The failure was the
    // detector's, not the render's.
    let numeral_changed = (0..3).all(|i| {
        (61..77).any(|x| {
            (15..31).any(|dy| {
                at_ench(&empty, x, dy + 19 * i) != at_ench(&unaffordable, x, dy + 19 * i)
            })
        })
    });
    c.record(
        "o5.an_offer_adds_a_numeral_where_an_empty_row_draws_none",
        numeral_changed,
        "all three numeral rects differ between an empty table and an unaffordable one, \
         which share a background",
    );

    // ...and the two really do share a background, or o5 is measuring that
    // instead. Probed at the row's left edge, outside the numeral's 16 px.
    let bg_same = (0..3).all(|i| {
        (60..61).all(|x| {
            (14..33).all(|dy| {
                at_ench(&empty, x, dy + 19 * i) == at_ench(&unaffordable, x, dy + 19 * i)
            })
        })
    });
    c.record(
        "o5b.the_two_frames_o5_compares_share_a_row_background",
        bg_same,
        "the row's left column is identical in both, so o5 isolates the numeral",
    );

    let offered = ench_frame([5, 10, 30], None, true, gpu, off, wr)?;

    // Hovering row 0 changes row 0 and leaves rows 1 and 2 alone. Creative is
    // on above, so all three rows are affordable and hoverable — otherwise the
    // hover branch is unreachable and this witness would be vacuous.
    let hovered = ench_frame([5, 10, 30], Some((100.0, 20.0)), true, gpu, off, wr)?;
    let row_changed = |i: i32, a: &[u8], b: &[u8]| {
        (60..168).any(|x| (14 + 19 * i..33 + 19 * i).any(|y| at_ench(a, x, y) != at_ench(b, x, y)))
    };
    c.record(
        "o6.a_hover_repaints_its_own_row_and_only_its_own",
        row_changed(0, &offered, &hovered)
            && !row_changed(1, &offered, &hovered)
            && !row_changed(2, &offered, &hovered),
        "row 0 differs under the cursor; rows 1 and 2 are byte-identical",
    );

    // -- the beacon ----------------------------------------------------------

    let beacon = rewo_world::menu_layout::layout_of(9).unwrap();
    let at_beacon = probe(beacon);
    let mut beacon_frame = |levels: i16,
                            primary: i16,
                            gpu: &mut Gpu,
                            off: &mut Offscreen,
                            wr: &mut WorldRenderer|
     -> Result<Vec<u8>, String> {
        let m = menu(9, &[(0, levels), (1, primary), (2, 0)]);
        let open = m.open().unwrap();
        wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
            open, 30, false, effects, None, None, None, None, None,
        ));
        wr.set_container(true, None);
        shot(gpu, off, wr)
    };

    // `tier < levels`: at 0 levels every power button is disabled, at 4 they
    // are all enabled, so the tier-0 button's chrome must differ.
    let dark = beacon_frame(0, 0, gpu, off, wr)?;
    let lit = beacon_frame(4, 0, gpu, off, wr)?;
    let tier0_differs =
        (53..75).any(|x| (22..44).any(|y| at_beacon(&dark, x, y) != at_beacon(&lit, x, y)));
    c.record(
        "o7.a_beacons_power_button_changes_chrome_with_its_level",
        tier0_differs,
        "the tier-0 button at (53, 22) 22x22 differs between 0 and 4 levels",
    );

    // The upgrade button is HIDDEN until a primary is chosen, so choosing one
    // paints something at (168, 47) that was not there before. Encoded, so
    // `1` is registry id 0 — the beacon's own `id + 1` convention.
    let no_primary = beacon_frame(4, 0, gpu, off, wr)?;
    let with_primary = beacon_frame(4, 1, gpu, off, wr)?;
    // Probe the button's 2 px CHROME ring, NOT the 18x18 icon it contains.
    //
    // The whole-button version was written first and a mutation proved it
    // vacuous: making the button always visible still changed these pixels,
    // because `beacon_upgrade_effect` reads `choice.primary` independently and
    // so the ICON appears either way. The claim is about the chrome, so the
    // probe has to be about the chrome.
    let ring = |img: &[u8]| -> Vec<[u8; 3]> {
        (168..190)
            .flat_map(|x| [(x, 47), (x, 48), (x, 67), (x, 68)])
            .chain((47..69).flat_map(|y| [(168, y), (169, y), (188, y), (189, y)]))
            .map(|(x, y)| at_beacon(img, x, y))
            .collect()
    };
    c.record(
        "o8.the_upgrade_buttons_CHROME_appears_only_once_a_primary_is_chosen",
        ring(&no_primary) != ring(&with_primary),
        "the 2px frame of the upgrade slot at (168, 47) is unpainted until primary is non-zero",
    );

    // -- M93j: the crafter's disabled-slot cover and redstone arrow --------
    //
    // The end-to-end half: M93i made a toggle correct on the wire and
    // invisible on screen. These grade that it reaches the frame.

    let crafter = rewo_world::menu_layout::layout_of(7).unwrap();
    let at_crafter = probe(crafter);
    let mut crafter_frame = |data: &[(i16, i16)],
                             gpu: &mut Gpu,
                             off: &mut Offscreen,
                             wr: &mut WorldRenderer|
     -> Result<Vec<u8>, String> {
        let m = menu(7, data);
        let open = m.open().expect("a menu must be open");
        wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
            open, 30, false, effects, None, None, None, None, None,
        ));
        wr.set_container(true, None);
        shot(gpu, off, wr)
    };

    // All nine enabled is the control. Grid slot 4 is the centre, at
    // (26 + 18, 17 + 18) = (44, 35), so its cover spans (43, 34)..(60, 51).
    let none_disabled = crafter_frame(&[], gpu, off, wr)?;
    let slot4_disabled = crafter_frame(&[(4, 1)], gpu, off, wr)?;

    // Measured, not assumed: the bounding box of everything that changed.
    let changed_box = |a: &[u8], b: &[u8]| -> Option<(i32, i32, i32, i32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for gy in 0..166i32 {
            for gx in 0..176i32 {
                if at_crafter(a, gx, gy) != at_crafter(b, gx, gy) {
                    x0 = x0.min(gx);
                    y0 = y0.min(gy);
                    x1 = x1.max(gx);
                    y1 = y1.max(gy);
                }
            }
        }
        (x0 <= x1).then_some((x0, y0, x1, y1))
    };
    let bbox = changed_box(&none_disabled, &slot4_disabled);
    c.record(
        "o9.disabling_a_slot_paints_an_18x18_cover_at_slot_minus_one",
        bbox.is_some_and(|(x0, y0, x1, y1)| {
            // Inside the declared 18x18 at (43, 34), and wide enough to be a
            // cover rather than a stray pixel. The far edges use `<=` because
            // the sprite may carry transparent margins.
            x0 >= 43 && y0 >= 34 && x1 <= 43 + 17 && y1 <= 34 + 17 && x1 - x0 >= 12
        }),
        format!(
            "changed bbox {bbox:?} — declared (43, 34)..(60, 51); the HIGHLIGHT's geometry              would be (40, 31)..(63, 54) and the ICON's (44, 35)..(59, 50)"
        ),
    );

    // ...and it lands on the slot that was disabled, not a fixed one.
    let slot0_disabled = crafter_frame(&[(0, 1)], gpu, off, wr)?;
    let bbox0 = changed_box(&none_disabled, &slot0_disabled);
    c.record(
        "o10.the_cover_follows_the_slot_that_was_disabled",
        bbox0.is_some_and(|(x0, y0, _, _)| x0 >= 25 && y0 >= 16 && x0 < 43),
        format!("slot 0 changed at {bbox0:?}, slot 4 at {bbox:?} — 18px apart per grid step"),
    );

    // The power flag shares the toggle array, and index 9 must move the ARROW
    // and cover no slot. Without the range guard on
    // `crafter_slot_disabled` this would paint a tenth cover as well.
    let powered = crafter_frame(&[(9, 1)], gpu, off, wr)?;
    let bbox9 = changed_box(&none_disabled, &powered);
    c.record(
        "o11.the_power_flag_swaps_the_arrow_and_covers_no_slot",
        bbox9.is_some_and(|(x0, y0, x1, y1)| {
            // The arrow is 16x16 at (97, 35); the grid ends at x = 80.
            x0 >= 97 && y0 >= 35 && x1 <= 97 + 15 && y1 <= 35 + 15
        }),
        format!("changed bbox {bbox9:?} — the arrow is (97, 35)..(112, 50), the grid ends at x=80"),
    );

    // o11 checks WHERE the arrow is and not WHICH arrow, so swapping the two
    // sprites survives it — both occupy the same 16x16 box. This pins the
    // choice, with the discriminating statistic MEASURED FROM THE ART.
    //
    // The first draft used luma and asserted the powered arrow is brighter.
    // It is not: measured, powered is luma 68 and unpowered 124, because a
    // lit redstone arrow is saturated red (0.299 * 255 = 76) against a pale
    // grey one. "Lit is brighter" is a perceptual intuition and luma is the
    // wrong statistic for it. REDNESS is what actually separates them, and
    // the witness derives the expectation rather than asserting it — which is
    // why the art was able to correct the premise instead of the premise
    // quietly inverting the witness.
    let redness = |rgba: &[u8]| -> f64 {
        let px: Vec<f64> = rgba
            .chunks_exact(4)
            .filter(|p| p[3] > 0)
            .map(|p| p[0] as f64 - p[1] as f64)
            .collect();
        if px.is_empty() {
            0.0
        } else {
            px.iter().sum::<f64>() / px.len() as f64
        }
    };
    // Read straight out of the bake: the point is an oracle from the ART.
    let overlay_sprites = &baked
        .container
        .as_ref()
        .ok_or("containershot: no container sprites in the bake")?
        .overlays;
    let sprite_redness = |i: usize| redness(&overlay_sprites[i].rgba);
    let off_red = sprite_redness(rewo_data::assets::CRAFTER_REDSTONE);
    let on_red = sprite_redness(rewo_data::assets::CRAFTER_REDSTONE + 1);
    // The rendered arrows over o11's box. Both frames share a background, so
    // the difference between them is driven by the sprite.
    let frame_redness = |f: &[u8]| -> f64 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for gy in 35..51i32 {
            for gx in 97..113i32 {
                let p = at_crafter(f, gx, gy);
                sum += p[0] as f64 - p[1] as f64;
                n += 1.0;
            }
        }
        sum / n
    };
    let unpowered_drawn = frame_redness(&none_disabled);
    let powered_drawn = frame_redness(&powered);
    c.record(
        "o12.the_powered_arrow_is_the_red_one_and_not_merely_a_different_sprite",
        // The art must separate on this statistic at all...
        on_red > off_red + 5.0
            // ...and the frames must order the same way, which a swap inverts.
            && powered_drawn > unpowered_drawn,
        format!(
            "sprite redness unpowered {off_red:.1} < powered {on_red:.1};              drawn unpowered {unpowered_drawn:.1} < powered {powered_drawn:.1}"
        ),
    );

    // o13 — the OTHER half of the cover, and the one a pixel witness here
    // cannot reach: `containershot` never calls `init_gui_items`, so the icon
    // pass is not running in these frames. This grades the production
    // `screen_icons` directly instead — the same function the live render
    // calls, on the M93b rule that a gate must exercise the derivation rather
    // than a copy of it.
    //
    // `extractSlot` calls `extractDisabledSlot` INSTEAD of `super`, so a
    // disabled slot draws no item. That is the opposite composition from the
    // toggle itself, which is additive (M93i).
    {
        let items = rewo_data::items::Items::load(
            &rewo_data::DataPaths::for_version("26.2")
                .ok_or("containershot: no data dir")?
                .registries_json(),
        )?;
        let stone = (0..)
            .take(4096)
            .find(|&i| items.name(i) == Some("minecraft:stone"))
            .ok_or("containershot: stone is not in the item registry")?;
        let stack = rewo_world::inventory::ItemSlot {
            item_id: stone,
            count: 1,
            has_components: false,
            components: 0,
            damage: None,
            max_damage: None,
            enchanted: false,
            any_enchantments: false,
            unbreakable: false,
            damage_component_removed: false,
            has_map_id: false,
            dye_removed: false,
            provides_banner_patterns_removed: false,
            trim_material: None,
        };
        // A crafter with the same stack in grid slots 0 and 4.
        let mut m = menu(7, &[]);
        let open = m.open_mut().expect("open");
        let mut content = vec![None; open.menu.slot_count()];
        content[0] = Some(stack);
        content[4] = Some(stack);
        assert!(open.menu.set_content(1, &content, None));

        let count = |m: &Menus| -> usize {
            let open = m.open().expect("open");
            crate::live_cmd::screen_icons(
                &open.menu,
                &items,
                &[],
                W as f32,
                H as f32,
                Some(open),
                None,
                None,
                // The book is shut for every icon witness here, and there is
                // no ghost recipe (M103) and no overlay (M104).
                None,
                &[],
                0,
                None,
            )
            .0
            .len()
        };
        let all_enabled = count(&m);
        // Disable slot 4 only.
        m.open_mut().expect("open").data[4] = 1;
        let one_disabled = count(&m);
        c.record(
            "o13.a_disabled_slot_draws_no_item",
            all_enabled == 2 && one_disabled == 1,
            format!(
                "two stacks in the grid: {all_enabled} icons with both enabled,                  {one_disabled} with slot 4 disabled — extractSlot replaces the                  slot's render rather than layering over it"
            ),
        );
    }

    // o14/o15 — the `gui.togglable_slot` hint, and the container-aware hover
    // that had to be fixed to make it reachable.
    {
        let lang = &baked.lang;
        let advance = &baked
            .font
            .as_ref()
            .ok_or("containershot: no baked font")?
            .advance;
        let items = rewo_data::items::Items::load(
            &rewo_data::DataPaths::for_version("26.2")
                .ok_or("containershot: no data dir")?
                .registries_json(),
        )?;
        let crafter_layout = rewo_world::menu_layout::layout_of(7).unwrap();
        // Grid slot 4 sits at (44, 35); its centre in screen pixels.
        let (left, top, scale) = rewo_gpu::container::gui_origin_for(
            W as f32,
            H as f32,
            crafter_layout.image_w as f32,
            crafter_layout.image_h as f32,
        );
        let over_slot4 = (
            (left + (44.0 + 8.0) * scale) as f64,
            (top + (35.0 + 8.0) * scale) as f64,
        );
        let hint = |m: &Menus, spectator: bool| -> bool {
            let open = m.open().expect("open");
            crate::live_cmd::screen_tooltip(
                &open.menu,
                &items,
                &baked.item_names,
                lang,
                &[],
                &baked.enchantment_text,
                &Default::default(),
                None,
                rewo_gpu::tooltip::TooltipFlag::NORMAL,
                advance,
                None,
                over_slot4,
                (W as f32, H as f32),
                crafter_layout,
                Some(open),
                spectator,
            )
            .is_some()
        };
        let enabled = menu(7, &[]);
        let disabled = menu(7, &[(4, 1)]);
        c.record(
            "o14.the_hint_is_on_an_ENABLED_slot_and_gone_once_it_is_disabled",
            hint(&enabled, false) && !hint(&disabled, false),
            format!(
                "hovering grid slot 4: enabled -> {}, disabled -> {} — the constant is                  named DISABLED_SLOT_TOOLTIP and reads {:?}, and it appears on the                  ENABLED one",
                hint(&enabled, false),
                hint(&disabled, false),
                lang.get("gui.togglable_slot")
            ),
        );
        c.record(
            "o15.a_spectator_gets_no_hint",
            !hint(&enabled, true),
            "the fifth condition, and the only one not otherwise observable here",
        );

        // Found by a surviving mutation: o14/o15 only ever hover a crafter's
        // GRID, so dropping `is_crafter_grid_slot` left every empty slot in
        // every menu offering to disable itself, and nothing could see it.
        let hint_at = |m: &Menus,
                       lay: &'static rewo_world::menu_layout::MenuLayout,
                       gui: (f64, f64)|
         -> bool {
            let open = m.open().expect("open");
            let (l, t, sc) = rewo_gpu::container::gui_origin_for(
                W as f32,
                H as f32,
                lay.image_w as f32,
                lay.image_h as f32,
            );
            crate::live_cmd::screen_tooltip(
                &open.menu,
                &items,
                &baked.item_names,
                lang,
                &[],
                &baked.enchantment_text,
                &Default::default(),
                None,
                rewo_gpu::tooltip::TooltipFlag::NORMAL,
                advance,
                None,
                (
                    (l + (gui.0 as f32 + 8.0) * sc) as f64,
                    (t + (gui.1 as f32 + 8.0) * sc) as f64,
                ),
                (W as f32, H as f32),
                lay,
                Some(open),
                false,
            )
            .is_some()
        };
        // A chest's slot 0, at (8, 18) in its own panel.
        let chest_layout = rewo_world::menu_layout::layout_of(2).unwrap();
        let chest = menu(2, &[]);
        // The crafter's own PLAYER slots start at slot 9; the standard block
        // is at (8, 84), so slot 9 sits there.
        let crafter_player = (8.0, 84.0);
        c.record(
            "o16.the_hint_is_confined_to_a_crafters_grid",
            !hint_at(&chest, chest_layout, (8.0, 18.0))
                && !hint_at(&enabled, crafter_layout, crafter_player)
                && hint_at(&enabled, crafter_layout, (44.0, 35.0)),
            format!(
                "chest slot 0 -> {}, crafter PLAYER slot -> {}, crafter GRID slot -> {}                  — without the grid gate every empty slot in every menu offers to                  disable itself",
                hint_at(&chest, chest_layout, (8.0, 18.0)),
                hint_at(&enabled, crafter_layout, crafter_player),
                hint_at(&enabled, crafter_layout, (44.0, 35.0))
            ),
        );

        // o17 — the OTHER half of the container-aware fix, and it needed a
        // different fixture to be observable at all.
        //
        // Reverting `screen_to_gui_for(layout)` to the player's 176x166
        // survives every witness above, because the CRAFTER's panel IS
        // 176x166: the two forms give identical answers for it. The panel size
        // only matters where the panel differs by more than a slot, which is
        // what `screen_to_gui_for`'s own doc says — a six-row chest is 176x222,
        // so its origin sits 28 px higher than the player's, more than the
        // 18 px pitch. An item tooltip is the probe, because an empty chest
        // has nothing to hover.
        let big = rewo_world::menu_layout::layout_of(5).unwrap();
        let mut chest6 = menu(5, &[]);
        {
            let open = chest6.open_mut().expect("open");
            let mut content = vec![None; open.menu.slot_count()];
            let stone = (0..)
                .take(4096)
                .find(|&i| items.name(i) == Some("minecraft:stone"))
                .ok_or("containershot: stone is not in the item registry")?;
            content[0] = Some(rewo_world::inventory::ItemSlot {
                item_id: stone,
                count: 1,
                has_components: false,
                components: 0,
                damage: None,
                max_damage: None,
                enchanted: false,
                any_enchantments: false,
                unbreakable: false,
                damage_component_removed: false,
                has_map_id: false,
                dye_removed: false,
                provides_banner_patterns_removed: false,
                trim_material: None,
            });
            assert!(open.menu.set_content(1, &content, None));
        }
        // Slot 0 of a chest sits at (8, 18) in ITS panel.
        let over = {
            let (l, t, sc) = rewo_gpu::container::gui_origin_for(
                W as f32,
                H as f32,
                big.image_w as f32,
                big.image_h as f32,
            );
            ((l + 16.0 * sc) as f64, (t + 26.0 * sc) as f64)
        };
        let open6 = chest6.open().expect("open");
        let got = crate::live_cmd::screen_tooltip(
            &open6.menu,
            &items,
            &baked.item_names,
            lang,
            &[],
            &baked.enchantment_text,
            &Default::default(),
            None,
            rewo_gpu::tooltip::TooltipFlag::NORMAL,
            advance,
            None,
            over,
            (W as f32, H as f32),
            big,
            Some(open6),
            false,
        );
        c.record(
            "o17.a_six_row_chests_hover_uses_ITS_panel_size_and_not_the_players",
            got.is_some(),
            format!(
                "a stack in slot 0 of a 176x{} panel resolves a tooltip; with the                  player's 166 the origin is {} px off — more than the 18 px slot pitch",
                big.image_h,
                (big.image_h - 166) / 2
            ),
        );
    }

    // o18 — the render honours the SCREEN's beacon choice, not just the
    // menu's. Found by a surviving mutation: every witness above drives the
    // menu's data slots, so reverting the render to `beacon_choice(m, ..)`
    // changed nothing any of them could see — and a click would have moved a
    // choice that nothing drew, which is M93i's "correct but invisible" one
    // screen over.
    {
        let beacon_layout = rewo_world::menu_layout::layout_of(9).unwrap();
        let at_beacon = probe(beacon_layout);
        let mut beacon_frame = |over: Option<rewo_world::menu_screen::BeaconChoice>,
                                gpu: &mut Gpu,
                                off: &mut Offscreen,
                                wr: &mut WorldRenderer|
         -> Result<Vec<u8>, String> {
            // levels 4 so every button is live, and a payment so Confirm is.
            let m = menu(9, &[(0, 4)]);
            let open = m.open().expect("open");
            wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
                open, 30, false, effects, None, over, None, None, None,
        ));
            wr.set_container(true, None);
            shot(gpu, off, wr)
        };
        let none = beacon_frame(None, gpu, off, wr)?;
        let picked = beacon_frame(
            Some(rewo_world::menu_screen::BeaconChoice {
                levels: 4,
                primary: Some(rewo_world::menu_screen::BeaconEffect::ALL[0]),
                secondary: None,
                has_payment: true,
            }),
            gpu,
            off,
            wr,
        )?;
        let differs = (0..166i32)
            .any(|y| (0..176i32).any(|x| at_beacon(&none, x, y) != at_beacon(&picked, x, y)));
        c.record(
            "o18.the_render_honours_the_screens_beacon_choice",
            differs,
            "a screen-local primary changes the frame — the selected button's chrome,              and the upgrade button becoming visible at all",
        );
    }

    // o19/o20 — M93q's solid-colour overlay quad, which the loom's grey banner
    // backing needs and the overlay path had no way to draw: `overlays` is
    // `(sprite, PanelBlit)`, and every sprite index samples the atlas.
    //
    // Graded here as a PRIMITIVE, with a hand-made overlay list. o21 below is
    // the matching USE, and the pair is deliberate: with only these two, the
    // whole loom arm could be deleted and both would still pass.
    {
        let brewing2 = rewo_world::menu_layout::layout_of(11).unwrap();
        let at_fill = probe(brewing2);
        // A cell of the brewing stand's flat panel, well clear of its slots.
        let cell = |rgb: u32| rewo_gpu::container::PanelBlit {
            dx: 60.0,
            dy: 20.0,
            w: 8.0,
            h: 8.0,
            sx: 0.0,
            sy: 0.0,
            sw: 0.0,
            sh: 0.0,
            tint: [
                ((rgb >> 16) & 0xFF) as f32 / 255.0,
                ((rgb >> 8) & 0xFF) as f32 / 255.0,
                (rgb & 0xFF) as f32 / 255.0,
                1.0,
            ],
        };
        let mut fill_frame = |overlays: Vec<(usize, rewo_gpu::container::PanelBlit)>,
                              gpu: &mut Gpu,
                              off: &mut Offscreen,
                              wr: &mut WorldRenderer|
         -> Result<[u8; 3], String> {
            let m = menu(11, &[]);
            let open = m.open().expect("open");
            let mut panel = crate::live_cmd::container_panel_for_open_menu(
                open, 30, false, effects, None, None, None, None, None,
        )
            .ok_or("containershot: no brewing panel")?;
            panel.overlays = overlays;
            wr.set_container_panel(Some(panel));
            wr.set_container(true, None);
            let img = shot(gpu, off, wr)?;
            // The middle of the 8x8 quad.
            Ok(at_fill(&img, 64, 24))
        };
        let bare = fill_frame(vec![], gpu, off, wr)?;
        let red = fill_frame(
            vec![(rewo_gpu::container::FILL_SPRITE, cell(0xFF0000))],
            gpu,
            off,
            wr,
        )?;
        let green = fill_frame(
            vec![(rewo_gpu::container::FILL_SPRITE, cell(0x00FF00))],
            gpu,
            off,
            wr,
        )?;
        c.record(
            "o19.a_fill_paints_its_own_colour_with_no_texture",
            red == [255, 0, 0] && bare != red,
            format!(
                "the centre of an 8x8 red fill reads {red:?} against {bare:?} with no fill — \
                 a textured quad here would sample the atlas at (0,0) instead"
            ),
        );
        c.record(
            "o20.the_fills_colour_comes_from_its_own_tint",
            green == [0, 255, 0] && green != red,
            format!("the same quad in green reads {green:?} where red read {red:?}"),
        );
    }

    // o21 — and the loom's arm actually EMITS one, under its pattern.
    //
    // The pair o19/o20 grades the primitive and this grades the use, because
    // `container_panel_for_open_menu` hardcoded `loom: None` until M93q and so
    // could not reach `menu_overlays`' loom arm at all — the shape M92 named
    // one level over: a gate that cannot reach a call site does not test it.
    //
    // The `LoomView` is SUPPLIED (resolving one needs an item registry this
    // gate has not got); `d7`–`d9` grade the item classification that feeds it
    // and `loom_pattern_table`'s own tests grade the sets.
    {
        let loom_layout = rewo_world::menu_layout::layout_of(18).unwrap();
        let at_loom = probe(loom_layout);
        // A solid quarter-field, so the pattern covers real area rather than a
        // hairline. Taken from the data, not spelled, so a retagged pack moves
        // the fixture with the table.
        let one: &'static [&'static str] = &rewo_data::loom_pattern_table::NO_ITEM_REQUIRED[..1];
        let pattern = one[0];
        let mut loom_frame = |display: bool,
                              gpu: &mut Gpu,
                              off: &mut Offscreen,
                              wr: &mut WorldRenderer|
         -> Result<Vec<u8>, String> {
            let m = menu(18, &[]);
            let open = m.open().expect("open");
            wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
                open,
                30,
                false,
                effects,
                None,
                None,
                Some(crate::live_cmd::LoomView {
                    display,
                    selected: 0,
                    start_row: 0,
                    patterns: one,
                }),
                None,
                None,
        ));
            wr.set_container(true, None);
            shot(gpu, off, wr)
        };
        let off_frame = loom_frame(false, gpu, off, wr)?;
        let on_frame = loom_frame(true, gpu, off, wr)?;
        // The preview rect of cell (0,0): 5x10 at (+4, +2) inside a 14 px cell
        // whose origin is the grid's.
        let (cx, cy) = rewo_world::menu_screen::loom_cell_origin(0, 0);
        let pv = rewo_world::menu_screen::loom_pattern_preview(cx, cy);
        let backing = rewo_world::menu_screen::LOOM_PREVIEW_BACKING;
        let grey = [
            ((backing >> 16) & 0xFF) as u8,
            ((backing >> 8) & 0xFF) as u8,
            (backing & 0xFF) as u8,
        ];
        let px = |img: &Vec<u8>| -> Vec<[u8; 3]> {
            (0..pv.h)
                .flat_map(|dy| (0..pv.w).map(move |dx| (dx, dy)))
                .map(|(dx, dy)| at_loom(img, pv.dx + dx, pv.dy + dy))
                .collect()
        };
        let on = px(&on_frame);
        let none = px(&off_frame);
        // The control first: without the grid, nothing here is the backing —
        // so the rect being probed really is the one the loom paints, and o21
        // is not reading some part of the sheet that was grey anyway.
        let control = !none.iter().any(|p| *p == grey);
        // The subject: the pattern sits OVER the fill. With the two pushes
        // swapped every pixel here would be exactly the backing.
        let lighter = on
            .iter()
            .filter(|p| p[0] > grey[0] && p[1] > grey[1] && p[2] > grey[2])
            .count();
        let greys = on.iter().filter(|p| **p == grey).count();
        c.record(
            "o21.the_looms_preview_paints_its_pattern_OVER_its_grey_backing",
            control && lighter > 0 && greys > 0,
            format!(
                "of {} preview pixels {lighter} are lighter than the {grey:?} backing and \
                 {greys} are exactly it, and with the grid hidden none of them is grey \
                 ({control}) — swap the two pushes and the pattern vanishes under the fill",
                on.len()
            ),
        );
    }

    // w1..w5 — the stonecutter's recipe grid (M93s).
    //
    // The `CutView` is SUPPLIED, as the loom's is and for the same reason: the
    // list is resolved from an item registry this gate has not got. What that
    // leaves untested is `cut_view`'s own resolution, which
    // `stonecutter_table`'s tests grade list-side.
    {
        use rewo_world::menu_screen as ms;
        let cut_layout = rewo_world::menu_layout::layout_of(24).unwrap();
        let at_cut = probe(cut_layout);
        // Six recipes, so the grid half-fills its second row and cannot scroll.
        let six: Vec<&'static rewo_data::stonecutter_table::Cut> =
            rewo_data::stonecutter_table::select_by_input("minecraft:andesite");
        assert!(six.len() == 6, "fixture: andesite offers {}", six.len());
        let mut cut_frame = |view: Option<crate::live_cmd::CutView>,
                             mouse: Option<(f64, f64)>,
                             gpu: &mut Gpu,
                             off: &mut Offscreen,
                             wr: &mut WorldRenderer|
         -> Result<Vec<u8>, String> {
            let m = menu(24, &[(0, -1)]);
            let open = m.open().expect("open");
            wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
                open,
                30,
                false,
                effects,
                mouse,
                None,
                None,
                view.as_ref(),
                None,
        ));
            wr.set_container(true, None);
            shot(gpu, off, wr)
        };
        let view = |selected: i32, start: i32, scroll: f32, n: usize| crate::live_cmd::CutView {
            recipes: six.iter().cycle().take(n).copied().collect(),
            start_index: start,
            selected,
            scroll_offs: scroll,
            display: true,
        };
        let hidden = cut_frame(None, None, gpu, off, wr)?;
        let shown = cut_frame(Some(view(-1, 0, 0.0, 6)), None, gpu, off, wr)?;

        // w1 — the grid draws at all, and only when `displayRecipes`.
        let cell0 = ms::cut_cell_sprite_origin(0);
        let lit = at_cut(&shown, cell0.0 + 8, cell0.1 + 9);
        let dark = at_cut(&hidden, cell0.0 + 8, cell0.1 + 9);
        c.record(
            "w1.the_grid_draws_only_when_display_recipes",
            lit != dark,
            format!(
                "cell 0 reads {lit:?} with a view and {dark:?} without — `hasInputItem` is \
                 both halves, so an unrecognised item shows no grid rather than an empty one"
            ),
        );

        // w2 — a partial last row stops at the list's end rather than drawing
        // twelve cells. Cell 6 is past six recipes.
        //
        // The control is a TWELVE-recipe view, not the bare panel. Comparing
        // against the bare panel is what the first cut did, and it was
        // UNSOUND: `recipe_selected.png`'s centre is (81, 73, 58), which is
        // exactly what `stonecutter.png` reads at this probe — so a cell 6
        // that wrongly drew a SELECTED chrome would have passed. The control
        // now differs from the test in one thing only: whether the list is
        // long enough to reach cell 6.
        let cell6 = ms::cut_cell_sprite_origin(6);
        let twelve = cut_frame(Some(view(-1, 0, 0.0, 12)), None, gpu, off, wr)?;
        let past = at_cut(&shown, cell6.0 + 8, cell6.1 + 9);
        let drawn = at_cut(&twelve, cell6.0 + 8, cell6.1 + 9);
        c.record(
            "w2.a_partial_last_row_draws_no_chrome_past_the_list",
            past != drawn,
            format!(
                "cell 6 reads {past:?} with six recipes and {drawn:?} with twelve — the loop                  breaks at `index >= size` rather than painting the whole page"
            ),
        );

        // w3 — the SELECTED cell's chrome differs from a plain one, which is
        // what makes data slot 0 observable at all.
        let picked = cut_frame(Some(view(0, 0, 0.0, 6)), None, gpu, off, wr)?;
        let sel = at_cut(&picked, cell0.0 + 8, cell0.1 + 9);
        let cell1 = ms::cut_cell_sprite_origin(1);
        let unsel = at_cut(&picked, cell1.0 + 8, cell1.1 + 9);
        c.record(
            "w3.the_selected_cell_wears_its_own_chrome",
            sel != lit && sel != unsel,
            format!(
                "with recipe 0 selected it reads {sel:?} against {lit:?} unselected and \
                 {unsel:?} for its neighbour — three-way, so `selected` beats `hovered`"
            ),
        );

        // w4 — the hover highlight keys off the ICON's box, two pixels below
        // the box a click uses. Hovering the click box's top two rows must
        // light the row ABOVE, not this one (M93s-b's shear).
        let (hx, _) = ms::cut_cell_origin(4);
        // `container_panel_for_open_menu` takes GUI pixels — the live path
        // hands it `screen_to_gui_for(...)` — so there is no conversion here.
        // The first cut of this witness converted the other way and the hover
        // simply never landed.
        //
        // gui y 32 is row 1's CLICK box and row 0's HIGHLIGHT box.
        let sheared = cut_frame(
            Some(view(-1, 0, 0.0, 6)),
            Some((hx as f64 + 8.0, 32.0)),
            gpu,
            off,
            wr,
        )?;
        let row0 = at_cut(&sheared, cell0.0 + 8, cell0.1 + 9);
        let row1 = at_cut(&sheared, ms::cut_cell_sprite_origin(4).0 + 8, ms::cut_cell_sprite_origin(4).1 + 9);
        c.record(
            "w4.the_hover_lights_the_row_ABOVE_the_one_a_click_would_hit",
            row0 != lit && row1 == lit,
            format!(
                "at gui y 32 — row 1's click box and row 0's highlight box — row 0 reads \
                 {row0:?} (changed from {lit:?}) and row 1 reads {row1:?} (unchanged). The \
                 two boxes tile on an 18 pitch, so the 2px offset is a shear, not a gap"
            ),
        );

        // w6 — and `selected` beats `hovered`, which w3 could not see because
        // it passes no mouse: the two orderings differ ONLY on a cell that is
        // both. Found by a surviving mutation that swapped the branches.
        //
        // `extractButtons` tests selected first, so hovering the recipe you
        // already picked keeps the selected chrome rather than flashing the
        // highlight over it.
        let (sx0, sy0) = ms::cut_cell_origin(0);
        let both = cut_frame(
            Some(view(0, 0, 0.0, 6)),
            Some((sx0 as f64 + 8.0, sy0 as f64 + 9.0)),
            gpu,
            off,
            wr,
        )?;
        let on_selected = at_cut(&both, cell0.0 + 8, cell0.1 + 9);
        c.record(
            "w6.hovering_the_selected_cell_keeps_the_selected_chrome",
            on_selected == sel && on_selected != row0,
            format!(
                "hovering the selected cell 0 reads {on_selected:?}, the SELECTED sprite                  {sel:?} and not the highlighted one {row0:?} — vanilla tests selected first"
            ),
        );

        // w5 — the scroller picks its sprite by `isScrollBarActive`, and moves.
        let short = cut_frame(Some(view(-1, 0, 0.0, 6)), None, gpu, off, wr)?;
        let long_top = cut_frame(Some(view(-1, 0, 0.0, 40)), None, gpu, off, wr)?;
        let long_bot = cut_frame(Some(view(-1, 28, 1.0, 40)), None, gpu, off, wr)?;
        let thumb = |img: &Vec<u8>, off_s: f32| {
            at_cut(
                img,
                ms::CUT_SCROLLER_X + 6,
                ms::cut_scroller_y(off_s) + 7,
            )
        };
        let a = thumb(&short, 0.0);
        let b = thumb(&long_top, 0.0);
        let moved = thumb(&long_bot, 0.0);
        c.record(
            "w5.the_scroller_is_disabled_on_a_short_list_and_travels_on_a_long_one",
            a != b && moved != b,
            format!(
                "the thumb reads {a:?} on a 6-recipe list (disabled sprite) against {b:?} on \
                 a 40-recipe one, and its old position reads {moved:?} once scrolled — 41 of \
                 travel against a 39 divisor, which is vanilla's own disagreement"
            ),
        );
    }

    // x1..x4 — the anvil's name field (M93t).
    //
    // The EditBox itself is graded by 19 unit tests; what these add is that the
    // RENDER reads it — the two cursor forms and the selection quad — through
    // the production `anvil_field_render`.
    {
        use rewo_world::edit_box as eb;
        let anvil_layout = rewo_world::menu_layout::layout_of(8).unwrap();
        let at_anvil = probe(anvil_layout);
        let mut field_frame = |setup: &dyn Fn(&mut eb::EditBox),
                               input: bool,
                               gpu: &mut Gpu,
                               off: &mut Offscreen,
                               wr: &mut WorldRenderer|
         -> Result<Vec<u8>, String> {
            let mut m = menu(8, &[]);
            if input {
                let o = m.open_mut().expect("open");
                let mut content = vec![None; o.menu.slot_count()];
                content[0] = Some(rewo_world::inventory::ItemSlot::plain(1, 1));
                assert!(o.menu.set_content(1, &content, None));
            }
            let open = m.open().expect("open");
            let mut field = eb::EditBox::new(rewo_world::anvil::MAX_NAME_LENGTH);
            field.set_focused(true);
            setup(&mut field);
            let mut panel = crate::live_cmd::container_panel_for_open_menu(
                open, 30, false, effects, None, None, None, None, None,
        )
            .ok_or("containershot: no anvil panel")?;
            // A 6-px monospace advance, so the geometry is arithmetic.
            let (_, fills, _) = crate::live_cmd::anvil_field_render_for_test(
                &field,
                &[6u8; 256],
                W as f32,
                H as f32,
            );
            panel.overlays.extend(fills);
            wr.set_container_panel(Some(panel));
            wr.set_container(true, None);
            shot(gpu, off, wr)
        };
        // The field's origin, from `AnvilScreen.subInit` — UNBORDERED, so the
        // text sits at the box's own corner rather than inset by 4.
        let (fx, fy) = (62, 24);
        let empty = field_frame(&|_| {}, false, gpu, off, wr)?;
        let typed = field_frame(&|f| f.set_value("abc"), false, gpu, off, wr)?;

        // x1 — `insert` is `cursorPos < len || len >= maxLength`, so a FULL
        // field shows the bar even with the cursor at the end.
        let full = field_frame(
            &|f| {
                f.set_max_length(3);
                f.set_value("abc");
            },
            false,
            gpu,
            off,
            wr,
        )?;
        let bar = at_anvil(&full, fx + 18, fy + 4);
        let none = at_anvil(&empty, fx + 18, fy + 4);
        c.record(
            "x1.a_full_field_shows_the_INSERT_bar_at_its_end",
            bar != none,
            format!(
                "a 3/3 field draws a quad at the cursor ({bar:?}) where an empty one has                  nothing ({none:?}) — the bar is how vanilla says there is no room left"
            ),
        );

        // x2 — and a field with room does not, because the append cursor is a
        // GLYPH the container pass never draws.
        let roomy = at_anvil(&typed, fx + 18, fy + 4);
        c.record(
            "x2.a_field_with_room_draws_no_cursor_QUAD",
            roomy == none,
            format!(
                "the same probe on a 3/50 field reads {roomy:?}, identical to empty — its                  cursor is the character `_`, not a rectangle"
            ),
        );

        // x3 — the selection is a blue quad over the highlighted run.
        let selected = field_frame(
            &|f| {
                f.set_value("abc");
                f.set_cursor_position(0);
                f.set_highlight_pos(3);
            },
            false,
            gpu,
            off,
            wr,
        )?;
        let mid = at_anvil(&selected, fx + 8, fy + 4);
        let mid_plain = at_anvil(&typed, fx + 8, fy + 4);
        c.record(
            "x3.a_selection_paints_a_blue_quad_over_its_run",
            mid[2] > mid[0] && mid[2] > mid[1] && mid != mid_plain,
            format!(
                "the middle of a fully-selected field reads {mid:?} against {mid_plain:?}                  unselected — `textHighlight` fills -16776961, pure blue"
            ),
        );

        // x5 — the field's BACKGROUND is a pair chosen by slot 0, and it is
        // load-bearing: `anvil.png` has a pure-red 255,0,0 band under it, so a
        // screen that omits the blit shows the placeholder. Found exactly that
        // way — the first run of these witnesses read [255, 0, 0] for the
        // "bare panel" and the sprite was missing.
        let with_item = field_frame(&|_| {}, true, gpu, off, wr)?;
        let enabled = at_anvil(&with_item, fx + 4, fy + 4);
        let disabled = at_anvil(&empty, fx + 4, fy + 4);
        // The values are the sprites' own, read out of the PNGs rather than
        // discovered from a frame: `text_field.png` is (160, 145, 114) and
        // `text_field_disabled.png` is (78, 71, 55) throughout their interiors.
        //
        // Naming them is the point. The first cut asserted only that the two
        // frames DIFFER, which is symmetric — inverting the pair swaps the two
        // readings and passes. A surviving mutation said so.
        c.record(
            "x5.the_field_background_is_chosen_by_slot_zero_and_covers_a_red_placeholder",
            enabled == [160, 145, 114] && disabled == [78, 71, 55],
            format!(
                "with an item the field reads {enabled:?} — `text_field.png` — and without it                  {disabled:?}, `text_field_disabled.png`. Neither is the sheet's red                  placeholder underneath, which is what the blit exists to cover"
            ),
        );

        // x4 — and it stops where the highlight does.
        let half = field_frame(
            &|f| {
                f.set_value("abcdef");
                f.set_cursor_position(0);
                f.set_highlight_pos(2);
            },
            false,
            gpu,
            off,
            wr,
        )?;
        let inside = at_anvil(&half, fx + 4, fy + 4);
        let outside = at_anvil(&half, fx + 28, fy + 4);
        c.record(
            "x4.the_selection_stops_where_the_highlight_does",
            inside[2] > inside[0] && outside == none,
            format!(
                "two of six units selected: inside reads {inside:?} and outside {outside:?},                  the bare panel — the quad spans cursor..highlight, not the field"
            ),
        );
    }

    // y1..y4 — the merchant's trade list (M93u).
    {
        use rewo_world::merchant_screen as ms;
        let mer_layout = rewo_world::menu_layout::layout_of(19).unwrap();
        let at_mer = probe(mer_layout);
        let offer = |out_of_stock: bool| rewo_net::merchant::MerchantOffer {
            cost_a: rewo_net::merchant::ItemCost {
                item_id: 1,
                count: 4,
                constrained: false,
            },
            // `special_price_diff` is inert here: the view carries the already
            // -resolved `cost_a_counts`, so a discount is expressed by making
            // those differ from `cost_a.count`.

            result: rewo_net::item_stack::WireSlot::Empty,
            cost_b: None,
            out_of_stock,
            uses: 0,
            max_uses: 16,
            xp: 2,
            special_price_diff: 0,
            price_multiplier: 0.05,
            demand: 0,
        };
        let mut mer_frame = |n: usize,
                             scroll: i32,
                             spent: bool,
                             bar: Option<(i32, i32, i32)>,
                             discounted: bool,
                             mouse: Option<(f64, f64)>,
                             gpu: &mut Gpu,
                             off: &mut Offscreen,
                             wr: &mut WorldRenderer|
         -> Result<Vec<u8>, String> {
            let m = menu(19, &[]);
            let open = m.open().expect("open");
            let offers: Vec<_> = (0..n).map(|i| offer(spent && i == 0)).collect();
            let view = crate::live_cmd::MerchantView {
                cost_a_counts: vec![if discounted { 1 } else { 4 }; n],
                offers,
                scroll_off: scroll,
                selected: 0,
                level: bar.map_or(5, |b| b.0),
                xp: bar.map_or(0, |b| b.1),
                show_progress: bar.is_some(),
                future_xp: bar.map_or(0, |b| b.2),
            };
            wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
                open,
                30,
                false,
                effects,
                mouse,
                None,
                None,
                None,
                Some(&view),
        ));
            wr.set_container(true, None);
            shot(gpu, off, wr)
        };
        // The trade arrow's centre, for row 0.
        // The arrow's centre for row 0. `COST_B_X + 20` — past cost B, which
        // is where the first cut of the ARM had it wrong too.
        let (ax, ay) = (ms::COST_B_X + 20 + 5, ms::row_item_y(0) + 3 + 4);
        let empty = mer_frame(0, 0, false, None, false, None, gpu, off, wr)?;
        let three = mer_frame(3, 0, false, None, false, None, gpu, off, wr)?;

        c.record(
            "y1.a_visible_offer_draws_its_trade_arrow",
            at_mer(&three, ax, ay) != at_mer(&empty, ax, ay),
            format!(
                "row 0 reads {:?} with three offers and {:?} with none",
                at_mer(&three, ax, ay),
                at_mer(&empty, ax, ay)
            ),
        );

        // y2 — a spent offer wears the out-of-stock arrow, a DIFFERENT sprite.
        // Probe the X's own arm, sprite (6, 2), where `trade_arrow.png` is
        // (198,198,198) and `trade_arrow_out_of_stock.png` is (150,35,0) —
        // read out of the PNGs rather than found by trial.
        //
        // This witness failed twice and BOTH times the fault was elsewhere.
        // First the arm drew the arrow at `5 + 5 + 20` where vanilla writes
        // `xo + 5 + 35 + 20`, putting every arrow 30 px left on top of the
        // cost-A icon — so the probe was reading bare panel. Then I explained
        // the survivor by claiming the two sprites share their centre; the
        // diff map says otherwise (row y=4 is `....XXX...`), and the centre
        // differs too. The lesson is the one M93s records: read the sprites
        // first, and do not invent a reason a witness passed.
        let spent = mer_frame(3, 0, true, None, false, None, gpu, off, wr)?;
        let (dx, dy) = (ms::COST_B_X + 20 + 6, ms::row_item_y(0) + 3 + 2);
        c.record(
            "y2.a_spent_offer_wears_a_different_arrow",
            at_mer(&spent, dx, dy) == [150, 35, 0] && at_mer(&three, dx, dy) == [198, 198, 198],
            format!(
                "at the X's arm a spent row reads {:?} — `trade_arrow_out_of_stock.png`'s                  own (150,35,0) — against {:?} in stock. Named rather than merely different,                  so swapping the pair cannot pass",
                at_mer(&spent, dx, dy),
                at_mer(&three, dx, dy)
            ),
        );

        // y3 — the scroller appears exactly when the list scrolls, which is
        // `steps > 1` reached by different arithmetic than `can_scroll`.
        let sx = ms::SCROLL_X + 3;
        let seven = mer_frame(7, 0, false, None, false, None, gpu, off, wr)?;
        let nine = mer_frame(9, 0, false, None, false, None, gpu, off, wr)?;
        let sy = ms::scroller_y(0, 9).expect("nine scrolls") + 13;
        c.record(
            "y3.the_scroller_appears_exactly_when_the_list_scrolls",
            at_mer(&nine, sx, sy) != at_mer(&seven, sx, sy) && ms::scroller_y(0, 7).is_none(),
            format!(
                "nine offers draw a thumb ({:?}) where seven do not ({:?}) — `steps > 1` is                  `size > 7`, the same threshold as `can_scroll` by other arithmetic",
                at_mer(&nine, sx, sy),
                at_mer(&seven, sx, sy)
            ),
        );

        // y5..y7 — the XP bar (M93v).
        {
            use rewo_world::merchant_screen as bar;
            let probe_x = |frac: f32| {
                bar::XP_BAR_X + ((bar::XP_BAR_W as f32 * frac) as i32).min(bar::XP_BAR_W - 1)
            };
            let by = bar::XP_BAR_Y + 2;
            // A wandering trader: `showProgressBar()` false, so no bar at all.
            let none = mer_frame(3, 0, false, None, false, None, gpu, off, wr)?;
            // Level 2 spans 10..70, so 40 xp is half the LEVEL — not half the
            // villager's career.
            let half = mer_frame(3, 0, false, Some((2, 40, 0)), false, None, gpu, off, wr)?;
            let (fill, _) = bar::xp_bar(2, 40, 0).expect("level 2 has a bar");
            assert_eq!(fill, 51, "fixture: half of 102");

            c.record(
                "y5.the_bar_is_drawn_only_when_the_merchant_has_a_level",
                at_mer(&half, probe_x(0.25), by) != at_mer(&none, probe_x(0.25), by),
                format!(
                    "a levelled trader draws {:?} where a wandering one draws {:?} —                      `showProgressBar()` gates the background too",
                    at_mer(&half, probe_x(0.25), by),
                    at_mer(&none, probe_x(0.25), by)
                ),
            );

            // y6 — the fill stops at its width: filled left of 51, background
            // right of it, and the two must differ.
            let inside = at_mer(&half, bar::XP_BAR_X + fill - 3, by);
            let outside = at_mer(&half, bar::XP_BAR_X + fill + 3, by);
            c.record(
                "y6.the_fill_stops_at_the_levels_own_fraction",
                inside != outside,
                format!(
                    "at 40 xp of level 2 (10..70) the bar is filled to {fill}/102: just                      inside reads {inside:?} and just outside {outside:?}"
                ),
            );

            // y7 — a master villager shows NOTHING, background included.
            let master = mer_frame(3, 0, false, Some((5, 999, 0)), false, None, gpu, off, wr)?;
            c.record(
                "y7.a_master_villager_shows_no_bar_at_all",
                at_mer(&master, probe_x(0.25), by) == at_mer(&none, probe_x(0.25), by)
                    && bar::xp_bar(5, 999, 0).is_none(),
                format!(
                    "level 5 reads {:?}, identical to no bar {:?} — `traderLevel < 5` gates                      the BACKGROUND, so a maxed villager is blank rather than full",
                    at_mer(&master, probe_x(0.25), by),
                    at_mer(&none, probe_x(0.25), by)
                ),
            );
            // y8 — the result segment samples the sprite from `w`, not 0, so
            // it CONTINUES the gradient where the fill stopped.
            //
            // Observable because both bar sprites are dark (14,17,16) at
            // x = 0 and x = 101 and flat in between — read out of the PNGs.
            // So the result segment's FIRST pixel is mid-gradient grey with
            // the offset and the sprite's dark left edge without it.
            let future = mer_frame(3, 0, false, Some((2, 40, 2)), false, None, gpu, off, wr)?;
            let (f2, fut) = bar::xp_bar(2, 40, 2).expect("level 2");
            assert!(fut > 1, "fixture: {fut} px of result");
            let first = at_mer(&future, bar::XP_BAR_X + f2, by);
            c.record(
                "y8.the_result_segment_continues_the_gradient_rather_than_restarting_it",
                first != [14, 17, 16],
                format!(
                    "the result segment's first pixel reads {first:?}; sampling from 0                      instead of {f2} would put the sprite's dark left edge (14,17,16) there"
                ),
            );
        }

        // z1..z3 — the trade button's chrome (M93x).
        {
            use rewo_world::merchant_screen as ms;
            let by = ms::button_y(0) + 10;
            let bare = mer_frame(0, 0, false, None, false, None, gpu, off, wr)?;
            let plain = mer_frame(3, 0, false, None, false, None, gpu, off, wr)?;
            // Hovering row 0's own 88x20 box. `container_panel_for_open_menu`
            // takes GUI pixels (M93t's correction).
            let hover_at = (
                (ms::TRADE_BUTTON_X + 40) as f64,
                (ms::button_y(0) + 10) as f64,
            );
            let hovered = mer_frame(3, 0, false, None, false, Some(hover_at), gpu, off, wr)?;
            // A pixel in the button's face, clear of the item icons.
            let face = ms::TRADE_BUTTON_X + 30;
            c.record(
                "z1.a_visible_offer_draws_a_button_under_its_items",
                at_mer(&plain, face, by) != at_mer(&bare, face, by),
                format!(
                    "the button's face reads {:?} with three offers and {:?} with none —                      drawn BEFORE the arrow and the icons, which sit on top of it",
                    at_mer(&plain, face, by),
                    at_mer(&bare, face, by)
                ),
            );
            // NAMED rather than merely different. The first cut asserted only
            // that the two frames differ, which is symmetric — inverting the
            // pair swaps the readings and passes. It was caught by z3 firing
            // instead of this one, which is why reading WHICH witness a
            // mutation kills matters as much as that one does. Same flaw
            // M93t's x5 had, and the values are the sprites' own: `button` is
            // (111,111,111) at this pixel and `button_highlighted` (117,117,117).
            c.record(
                "z2.hovering_a_row_swaps_the_button_sprite",
                at_mer(&plain, face, by) == [111, 111, 111]
                    && at_mer(&hovered, face, by) == [117, 117, 117],
                format!(
                    "plain reads {:?} and hovered {:?} — `button_highlighted` is six values                      lighter throughout, and `TradeOfferButton` is never INACTIVE, so the                      disabled sprite is unreachable",
                    at_mer(&plain, face, by),
                    at_mer(&hovered, face, by)
                ),
            );
            // The right corner: destination x 87 samples source x 199, the
            // sheet's black border. A naive 1:1 blit of x 0..88 — the obvious
            // wrong implementation — would put source x 87 (a mid-face 112)
            // there instead, read out of the PNG.
            let right = at_mer(&plain, ms::TRADE_BUTTON_X + ms::TRADE_BUTTON_W - 1, by);
            c.record(
                "z3.the_right_corner_is_the_SHEETS_right_edge_not_a_mid_face_pixel",
                right[0] < 60,
                format!(
                    "the button's last column reads {right:?}; source x 199 is the sheet's                      black border, where a 1:1 blit from x 0 would sample x 87 — (112,112,112)"
                ),
            );
        }

        // y9 — the discounted price pair (M93w): one icon, TWO numbers, and a
        // strikethrough through the first.
        {
            use rewo_world::merchant_screen as ms;
            let plain = mer_frame(3, 0, false, None, false, None, gpu, off, wr)?;
            let cut = mer_frame(3, 0, false, None, true, None, gpu, off, wr)?;
            // The strikethrough's own row, at COST_A_X + 7, item y + 12.
            let (sx, sy) = (
                ms::COST_A_X + ms::STRIKETHROUGH_DX + 4,
                ms::row_item_y(0) + ms::STRIKETHROUGH_DY,
            );
            let struck = at_mer(&cut, sx, sy);
            let unstruck = at_mer(&plain, sx, sy);
            c.record(
                "y9.a_discounted_price_strikes_through_its_base_number",
                struck != unstruck,
                format!(
                    "the strikethrough row reads {struck:?} discounted against {unstruck:?}                      plain — drawn at COST_A_X + 7, through the FIRST number rather than in                      the gap between the two"
                ),
            );
            // The two DIGITS are not witnessed here, and the reason is
            // structural rather than an omission: `mer_frame` builds the
            // PANEL, and the count labels come from `screen_icons`, which this
            // gate never calls. Probing for them found 0 changed pixels over a
            // 10x10 cell — correctly, because nothing in this frame draws
            // text. M45's shape: a gate reimplementing a slice of the app's
            // setup misses whatever lives outside it.
            //
            // They are graded at the model level instead — `y10` below and
            // `merchant_screen`'s three `cost_a_display` tests — which covers
            // the rules (which digits, forced or not) and not their pixels.

            // y10 — and the second number is a FORCED "1": vanilla's
            // `countText` override exists to defeat `itemCount`'s own rule
            // that a single item draws no digit at all.
            let one = ms::cost_a_display(4, 1);
            c.record(
                "y10.the_discounted_digit_is_forced_even_when_it_is_a_one",
                one.at_second == Some(1) && ms::cost_a_display(1, 1).at_icon.is_none(),
                format!(
                    "a 4 -> 1 discount draws {:?} as its second digit, while an undiscounted                      single item draws {:?} — the `count == 1 ? \"1\" : null` override is the                      whole difference",
                    one.at_second,
                    ms::cost_a_display(1, 1).at_icon
                ),
            );
        }

        // y4 — scrolling moves WHICH offers are drawn, not where the rows are.
        // With 9 offers and scroll 2, offer 8 occupies row 6.
        let scrolled = mer_frame(9, 2, false, None, false, None, gpu, off, wr)?;
        let bottom = (ax, ms::row_item_y(6) + 3 + 4);
        let unscrolled_bottom = at_mer(&nine, bottom.0, bottom.1);
        let scrolled_bottom = at_mer(&scrolled, bottom.0, bottom.1);
        c.record(
            "y4.the_window_slides_over_the_offers_and_the_rows_stay_put",
            unscrolled_bottom == scrolled_bottom
                && ms::offer_visible(8, 2, 9)
                && !ms::offer_visible(8, 0, 9),
            format!(
                "row 6 draws an arrow either way ({unscrolled_bottom:?} /                  {scrolled_bottom:?}) — the ROW is fixed and the offer in it changes, so                  offer 8 is invisible at scroll 0 and visible at scroll 2"
            ),
        );
    }

    // -- the recipe book (M94) ----------------------------------------------
    //
    // Two origins are in play: the menu's, which an open book MOVES, and the
    // book's own, which is anchored to the window.
    {
        use rewo_world::recipe_book_screen as rb;
        let (bl, bt, bsc) = rewo_gpu::container::recipe_book_origin(W as f32, H as f32);
        let book_items = rewo_data::items::Items::load(
            &rewo_data::DataPaths::for_version("26.2")
                .ok_or("containershot: no data dir")?
                .registries_json(),
        )?;
        let dirt_id = book_items.id("dirt").expect("dirt");
        let at_book = |img: &[u8], gx: i32, gy: i32| -> [u8; 3] {
            let x = (bl + (gx as f32 + 0.5) * bsc) as u32;
            let y = (bt + (gy as f32 + 0.5) * bsc) as u32;
            let i = ((y.min(H - 1) * W + x.min(W - 1)) * 4) as usize;
            [img[i], img[i + 1], img[i + 2]]
        };
        let book = |shown: usize, total: usize, page: usize| crate::live_cmd::BookRender {
            view: Some(rb::BookView {
                // The book's OWN count. Hard-coding 4 here was M93z's error
                // surviving in the fixture: a crafting book has five tabs, and
                // b8 measured 26 icons against the 27 it named.
                tabs: rb::BookType::Crafting.tabs().len(),
                selected_tab: 0,
                page,
                total_pages: total,
                shown,
                filtering: false,
                furnace_family: false,
            }),
            // TWENTY slots whatever the page shows, which is vanilla's own
            // structure: `updateButtonsForPage` keeps 20 `RecipeButton`s and
            // flips `visible` on the ones past the end. Sizing this vec to
            // `shown` instead would make `book_chrome`'s `take(shown)` a no-op
            // here, and a mutation deleting it survived the gate for exactly
            // that reason - caught by the model's own test, whose fixture does
            // pass 20.
            slots: vec![(true, false); rb::ITEMS_PER_PAGE],
            hover: rb::BookHover::default(),
            book: rb::BookType::Crafting,
            // A dirt icon per visible cell, and no shadow copies — the item
            // witnesses vary these where they need to.
            slot_items: vec![Some(dirt_id); rb::ITEMS_PER_PAGE],
            slot_shadowed: vec![false; rb::ITEMS_PER_PAGE],
            slot_recipes: vec![None; rb::ITEMS_PER_PAGE],
            // M104 — the which-of-these overlay's source material. Empty
            // here: this closure builds a book with no OPEN overlay, and the
            // overlay witnesses supply their own snapshot.
            slot_collections: vec![Vec::new(); rb::ITEMS_PER_PAGE],
        };
        // The crafting table - one of the four menus that actually has a book.
        // Menu type 12; 13 is `enchantment`, which an earlier draft of this
        // named `craft` and which is the same 176x166 size, so nothing here
        // noticed. Only `live --render-check` did, by opening it and finding
        // no book.
        let craft = rewo_world::menu_layout::layout_of(12).unwrap();
        // M104 — the open overlay and the cursor, carried on the SAME entry
        // point rather than a second one, for the reason M93q recorded: a gate
        // that cannot reach a call site does not test it, and one that reaches
        // it through its own copy of the setup misses whatever the app adds
        // (M45). Both are `None` for every witness written before M104, so
        // nothing already green moves.
        let mut book_frame = |b: Option<&crate::live_cmd::BookRender>,
                              overlay: Option<&rewo_world::recipe_overlay::Open>,
                              book_mouse: Option<(i32, i32)>,
                              gpu: &mut Gpu,
                              off: &mut Offscreen,
                              wr: &mut WorldRenderer|
         -> Result<Vec<u8>, String> {
            let m = menu(12, &[]);
            wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
                m.open().unwrap(),
                30,
                false,
                effects,
                None,
                None,
                None,
                None,
                None,
            ));
            // The book is set BESIDE the panel, not inside it — the seam
            // `--render-check`'s r23 forced, because the player's own
            // inventory has no `ContainerPanel` and still has a book.
            // The gate drives no search field (M100), so the book draws its
            // chrome and no field quads.
            wr.set_recipe_book(
                b.and_then(|x| crate::live_cmd::recipe_book_panel(x, &[], overlay, book_mouse)),
            );
            wr.set_container(true, None);
            shot(gpu, off, wr)
        };
        let shut = book_frame(None, None, None, gpu, off, wr)?;
        let full = book(20, 1, 0);
        let open = book_frame(Some(&full), None, None, gpu, off, wr)?;
        // Taken here so `book_frame` can be dropped before b4, which needs
        // `shot` itself.
        let short = book(5, 3, 2);
        let short_frame = book_frame(Some(&short), None, None, gpu, off, wr)?;
        let uncraftable_slots = crate::live_cmd::BookRender {
            slots: vec![(false, false); 20],
            ..full.clone()
        };
        let unc_frame = book_frame(Some(&uncraftable_slots), None, None, gpu, off, wr)?;
        // M104's frames, taken here for the same reason the four above are:
        // `book_frame` borrows `shot` and must be dropped before b4 uses it.
        use rewo_world::recipe_overlay as ro;
        let opened = |flags: &[bool], furnace: bool| ro::Open {
            // Anchored on the cell a right-click on slot 0 would open it from,
            // which is inside every clamp - the clamps have their own witnesses
            // in the model, and pinning them again here would only measure
            // `open_overlay` twice.
            origin: rb::grid_slot(0),
            furnace,
            buttons: flags
                .iter()
                .enumerate()
                .map(|(i, &c)| ro::Button { recipe: i as i32, craftable: c, slots: Vec::new() })
                .collect(),
        };
        let two = opened(&[true, false], false);
        let (ox, oy) = two.origin;
        // Button `i` of an `n`-button overlay, offset into it.
        let btn = |i: usize, n: usize, dx: i32, dy: i32| {
            let (bx, by) = ro::button_origin((ox, oy), i, n);
            (bx + dx, by + dy)
        };
        // Named `ov0*` rather than `c0*`: this block already binds `(c0x, c0y)`
        // to `grid_slot(0)` further down, and the first cut of these witnesses
        // was shadowed by it - so o2, o3 and o4 all probed the recipe CELL's
        // top-left corner and read the 204 of `slot_craftable`'s corner while
        // the render was correct all along.
        let (ov0x, ov0y) = btn(0, 2, 12, 12);
        let ov = book_frame(Some(&full), Some(&two), None, gpu, off, wr)?;
        let hov = book_frame(Some(&full), Some(&two), Some((ov0x, ov0y)), gpu, off, wr)?;
        let fur =
            book_frame(Some(&full), Some(&opened(&[true, false], true)), None, gpu, off, wr)?;
        let five = opened(&[true; 5], false);
        let wide = book_frame(Some(&full), Some(&five), None, gpu, off, wr)?;
        let deep = book_frame(Some(&full), Some(&opened(&[true; 8], false)), None, gpu, off, wr)?;
        drop(book_frame);

        // b1 - the book paints its own sheet.
        //
        // The value is NAMED - `recipe_book.png`'s own (55,55,55), read out of
        // the PNG - rather than merely "different from shut", and both flaws
        // that forced the rewrite are why. The first draft probed book(73,80)
        // and compared open against shut: it passed, reading [0,0,0] against
        // [255,255,255], and neither number was the book. (73,80) is inside
        // recipe slot 7, near its BLACK bottom border; and the shut frame had
        // the menu panel over that screen position, because an open book MOVES
        // the menu - so the control changed along with the subject, which is
        // the one thing a frame diff may not do. book(10,80) is bare panel and
        // is outside the menu in both frames.
        c.record(
            "b1.an_open_book_paints_its_own_sheet",
            at_book(&open, 10, 80) == [55, 55, 55] && at_book(&shut, 10, 80) != [55, 55, 55],
            format!(
                "bare panel reads {:?} - `recipe_book.png` at (11,81), its own value - against {:?} with the book shut",
                at_book(&open, 10, 80),
                at_book(&shut, 10, 80)
            ),
        );

        // b2 - and it MOVES the menu. Measured against `screen_left`'s own
        // arithmetic rather than by eye: 77 GUI px for a 176-wide panel.
        let sw = W as f32 / bsc;
        let centred = rb::screen_left(sw as i32, craft.image_w, false, false);
        let shifted = rb::screen_left(sw as i32, craft.image_w, true, false);
        let place = |open_book: bool| {
            rewo_gpu::container::gui_origin_placed(
                W as f32,
                H as f32,
                rewo_gpu::container::Placement::with_book(
                    craft.image_w as f32,
                    craft.image_h as f32,
                    open_book,
                ),
            )
        };
        let (ml, mt, _) = place(true);
        let (ml0, mt0, _) = place(false);
        c.record(
            "b2.an_open_book_shifts_the_menu_by_screen_lefts_own_amount",
            shifted - centred == 77 && ((ml - ml0) / bsc - 77.0).abs() < 1e-3,
            format!(
                "the model says {centred} -> {shifted} and the renderer moves the panel {:.1} GUI px",
                (ml - ml0) / bsc
            ),
        );

        // b3 - the shift is horizontal ONLY. `topPos` is untouched in vanilla,
        // and a symmetric "re-centre both axes" reading would move it too.
        c.record(
            "b3.the_shift_is_horizontal_only",
            mt == mt0,
            format!("top is {mt} either way"),
        );

        // b4 - the panel is sampled from (1, 1). Proven by rendering the SAME
        // book with a (0, 0) source and showing the frames differ: without
        // that the constant could be anything and no pixel would notice. The
        // constant itself is pinned by a unit test; this is the half that says
        // it reaches the draw.
        let mut zeroed = crate::live_cmd::recipe_book_panel(&full, &[], None, None).unwrap();
        for b in &mut zeroed.blits {
            b.sx = 0.0;
            b.sy = 0.0;
        }
        wr.set_recipe_book(Some(zeroed));
        wr.set_container(true, None);
        let from_zero = shot(gpu, off, wr)?;
        let differs = (0..(W * H) as usize).any(|i| {
            from_zero[i * 4] != open[i * 4] || from_zero[i * 4 + 1] != open[i * 4 + 1]
        });
        c.record(
            "b4.the_panels_one_one_source_offset_reaches_the_draw",
            differs && rb::PANEL_SOURCE == (1, 1),
            "sourcing the panel at (0,0) instead of (1,1) changes the frame".to_string(),
        );

        // b5 - the tab column hangs off the book's LEFT edge, and only the
        // SELECTED tab wears the selected sprite.
        //
        // Both sprites are named: `tab.png` is grey 139 at its middle and
        // `tab_selected.png` is 198, read out of the PNGs. That is what makes
        // this witness able to fail - "the tabs differ from the backdrop" would
        // pass with every tab drawing the same art, which is exactly the
        // mistake `sprites.get(true, this.selected)` invites.
        let tab_mid = |i: i32, sel: bool| {
            (
                rb::TAB_DX + rb::tab_x_shift(sel) + 17,
                rb::TAB_DY + rb::TAB_PITCH * i + 13,
            )
        };
        let (t0x, t0y) = tab_mid(0, true);
        let (t1x, t1y) = tab_mid(1, false);
        c.record(
            "b5.only_the_selected_tab_wears_the_selected_sprite",
            rb::TAB_DX < 0
                && at_book(&open, t0x, t0y) == [198, 198, 198]
                && at_book(&open, t1x, t1y) == [139, 139, 139],
            format!(
                "tab 0 (selected) reads {:?} - `tab_selected.png`'s own 198 - and tab 1 reads {:?}, `tab.png`'s 139; the column sits at book x={}, outside the 0..147 panel",
                at_book(&open, t0x, t0y),
                at_book(&open, t1x, t1y),
                rb::TAB_DX
            ),
        );

        // b7 - a slot's chrome follows its collection's craftable flag, and
        // both sprites are named: `slot_craftable.png` is 139 at its middle
        // and `slot_uncraftable.png` is 106.
        let (c0x, c0y) = rb::grid_slot(0);
        c.record(
            "b7.a_slots_chrome_names_its_craftable_state",
            at_book(&open, c0x + 12, c0y + 12) == [139, 139, 139]
                && at_book(&unc_frame, c0x + 12, c0y + 12) == [106, 106, 106],
            format!(
                "slot 0 reads {:?} craftable and {:?} not - `slot_craftable.png`'s 139 against `slot_uncraftable.png`'s 106",
                at_book(&open, c0x + 12, c0y + 12),
                at_book(&unc_frame, c0x + 12, c0y + 12)
            ),
        );

        // b8 - the book's ITEMS. Counted rather than probed for a pixel: an
        // icon is a 3D model rendered by another pass, so its colour at a given
        // texel is a property of the model and the GUI lighting rather than of
        // the placement this milestone is about.
        //
        // The count is exact and each term is named, so a change in either
        // half shows: five tabs, of which two carry a pair, is seven; plus one
        // per visible slot.
        // M104 — the overlay is carried on the SAME closure the earlier
        // witnesses use, not a second one, so a gate cannot exercise a path the
        // live client does not take (M45/M93q). Every pre-M104 call passes
        // `None`, so nothing already green moves.
        let icons_of = |b: Option<&crate::live_cmd::BookRender>,
                        o: Option<&rewo_world::recipe_overlay::Open>|
         -> usize {
            let inv = rewo_world::inventory::Inventory::default();
            crate::live_cmd::screen_icons(
                &inv,
                &book_items,
                &[],
                W as f32,
                H as f32,
                None,
                None,
                None,
                b,
                &[],
                0,
                o,
            )
            .0
            .len()
        };
        let base = icons_of(None, None);
        let with_book = icons_of(Some(&full), None);
        c.record(
            "b8.the_book_draws_an_icon_per_tab_and_per_visible_slot",
            with_book - base == 7 + rb::ITEMS_PER_PAGE,
            format!(
                "{} icons over the shut-book baseline of {base}: 5 crafting tabs of which 2 carry a pair = 7, plus one per visible slot",
                with_book - base
            ),
        );

        // b9 - a shadowed slot draws TWO icons of the same item, not one.
        let shadowed = crate::live_cmd::BookRender {
            slot_shadowed: vec![true; rb::ITEMS_PER_PAGE],
            ..full.clone()
        };
        c.record(
            "b9.a_shadowed_slot_draws_the_display_stack_twice",
            icons_of(Some(&shadowed), None) - with_book == rb::ITEMS_PER_PAGE,
            format!(
                "shadowing all 20 slots adds exactly {} icons - one per slot, the same stack drawn again",
                icons_of(Some(&shadowed), None) - with_book
            ),
        );

        // b10 - a slot whose result Rewo cannot resolve draws NO icon, rather
        // than a placeholder. `SlotDisplay::Tag` and friends need a context
        // Rewo has not got, and an arbitrary tag member would be a confident
        // wrong answer.
        let unresolved = crate::live_cmd::BookRender {
            slot_items: vec![None; rb::ITEMS_PER_PAGE],
            ..full.clone()
        };
        c.record(
            "b10.an_unresolvable_result_draws_nothing_rather_than_a_guess",
            with_book - icons_of(Some(&unresolved), None) == rb::ITEMS_PER_PAGE,
            format!(
                "dropping all 20 results removes exactly {} icons and leaves the 7 tab icons",
                with_book - icons_of(Some(&unresolved), None)
            ),
        );

        // b11 - the tab icons come from the BOOK's own list, whose length
        // differs per book: crafting 5 (2 paired) = 7 icons, smoker 2 (0
        // paired) = 2. M94 assumed four tabs for every book.
        let smoker = crate::live_cmd::BookRender {
            book: rb::BookType::Smoker,
            view: Some(rb::BookView {
                tabs: rb::BookType::Smoker.tabs().len(),
                ..full.view.unwrap()
            }),
            ..full.clone()
        };
        c.record(
            "b11.each_book_draws_its_OWN_tab_list",
            icons_of(Some(&smoker), None) - base == 2 + rb::ITEMS_PER_PAGE
                && rb::BookType::Crafting.tabs().len() == 5
                && rb::BookType::Smoker.tabs().len() == 2,
            format!(
                "a smoker book adds {} icons against a crafting book's {} - 2 tabs neither of which is paired, against 5 tabs of which 2 are",
                icons_of(Some(&smoker), None) - base,
                with_book - base
            ),
        );

        // b12 / b13 - WHERE the icons land.
        //
        // b8..b11 count icons, and a count cannot see a wrong origin: two
        // mutations survived them, one putting the book's icons on the menu's
        // origin and one leaving the menu's icons on a centred origin while an
        // open book moved the panel. Both are the failure this milestone exists
        // to avoid, and both need a position, so here it is.
        let icons_at = |b: Option<&crate::live_cmd::BookRender>,
                        inv: &rewo_world::inventory::Inventory,
                        o: Option<&rewo_world::recipe_overlay::Open>|
         -> Vec<rewo_gpu::gui_item::GuiItem> {
            crate::live_cmd::screen_icons(
                inv,
                &book_items,
                &[],
                W as f32,
                H as f32,
                None,
                None,
                None,
                b,
                &[],
                0,
                o,
            )
            .0
        };
        let empty_inv = rewo_world::inventory::Inventory::default();
        let placed = icons_at(Some(&full), &empty_inv, None);
        // Slot 0's icon, which `book_icons` puts at grid_slot(0) + 4.
        let (g0x, g0y) = rb::grid_slot(0);
        let want = (bl + (g0x + 4) as f32 * bsc, bt + (g0y + 4) as f32 * bsc);
        c.record(
            "b12.the_books_icons_sit_on_the_BOOKs_origin",
            placed
                .iter()
                .any(|i| (i.x - want.0).abs() < 0.01 && (i.y - want.1).abs() < 0.01),
            format!(
                "an icon lands at {want:?}, the book's origin plus grid slot 0 plus 4 - the menu's origin is {:?}, which is where the surviving mutation put it",
                rewo_gpu::container::gui_origin(W as f32, H as f32)
            ),
        );

        // b13 - and the MENU's icons move with the panel when the book opens.
        //
        // A stocked player inventory, so there is an icon to find; slot 36 is
        // the first hotbar slot.
        // The menu's icon is told apart from the book's by its ITEM, not by
        // where it is: the book's cells all hold dirt, so a sword in the menu
        // is unambiguous. Filtering by position would be circular, since
        // position is exactly what this measures - and the first draft did
        // that and found nothing, because the book's right edge is right of
        // the player panel's left edge.
        let sword_id = book_items.id("diamond_sword").expect("diamond_sword");
        let sword = book_items.name(sword_id).expect("named").to_string();
        let mut stocked = rewo_world::inventory::Inventory::default();
        stocked.set_slot(0, 36, Some(rewo_world::inventory::ItemSlot::plain(sword_id, 1)));
        let shut_icons = icons_at(None, &stocked, None);
        let open_icons = icons_at(Some(&full), &stocked, None);
        let first_menu_icon = |v: &[rewo_gpu::gui_item::GuiItem]| -> Option<f32> {
            v.iter().find(|i| i.model == sword).map(|i| i.x)
        };
        let shift = match (first_menu_icon(&shut_icons), first_menu_icon(&open_icons)) {
            (Some(a), Some(b)) => Some((b - a) / bsc),
            _ => None,
        };
        c.record(
            "b13.the_menus_icons_move_with_the_panel_the_book_pushed",
            shift.is_some_and(|d| (d - 77.0).abs() < 0.01),
            format!(
                "a menu icon moves {:?} GUI px when the book opens - the same 77 the panel moves in b2, because the icon is measured from the panel's origin",
                shift
            ),
        );

        // b6 - a short page draws fewer slots, and the ones it drops go back to
        // bare panel rather than staying lit.
        let (sx, sy) = rb::grid_slot(19);
        let (fx, fy) = rb::grid_slot(0);
        c.record(
            "b6.a_short_page_draws_only_the_slots_it_has",
            at_book(&open, sx + 12, sy + 12) == [139, 139, 139]
                && at_book(&short_frame, sx + 12, sy + 12) == [55, 55, 55]
                && at_book(&short_frame, fx + 12, fy + 12)
                    == at_book(&open, fx + 12, fy + 12),
            format!(
                "slot 19 reads {:?} on a full page and {:?} on a 5-slot one - the sheet's own bare panel, so the cell is not merely dimmed - while slot 0 is identical in both",
                at_book(&open, sx + 12, sy + 12),
                at_book(&short_frame, sx + 12, sy + 12)
            ),
        );

        // --- M104, the which-of-these overlay ------------------------------
        //
        // Every witness here names the value it expects, read out of the
        // sprite PNG, rather than "different from the backdrop" - M94's
        // lesson, and it matters more here than usual because a plain overlay
        // BUTTON and a craftable recipe CELL are the same 139 grey. Telling
        // those apart by "not the panel" would pass with the two swapped.
        //
        //   overlay_recipe            interior   198
        //   crafting_overlay          centre     139  and 55 at (8, 2)
        //   crafting_overlay_disabled centre     147,107,107
        //   ..._highlighted           centre     136,146,201
        //   furnace_overlay           centre     139  and 139 at (8, 2)
        // o1 - the panel draws, and it draws OVER the cell it opened from.
        //
        // Button 1 is the UNCRAFTABLE one, so its reddish centre cannot be
        // confused with the 139 grey of either a craftable button or the
        // recipe cell underneath - which is the confusion this fixture is
        // arranged to avoid.
        let (ov1x, ov1y) = btn(1, 2, 12, 12);
        c.record(
            "o1.the_overlay_covers_the_cell_that_opened_it",
            at_book(&ov, ov1x, ov1y) == [147, 107, 107]
                && at_book(&open, ov1x, ov1y) != [147, 107, 107],
            format!(
                "the uncraftable button reads {:?} where the book alone reads {:?} - `crafting_overlay_disabled`, over a cell",
                at_book(&ov, ov1x, ov1y),
                at_book(&open, ov1x, ov1y)
            ),
        );

        // o2 - craftable and uncraftable buttons wear different art, and the
        // craftable one is the plain sprite rather than the panel showing
        // through.
        c.record(
            "o2.a_craftable_button_and_an_uncraftable_one_differ",
            at_book(&ov, ov0x, ov0y) == [139, 139, 139]
                && at_book(&ov, ov1x, ov1y) == [147, 107, 107],
            format!(
                "button 0 reads {:?} (`crafting_overlay`) and button 1 {:?} (`..._disabled`)",
                at_book(&ov, ov0x, ov0y),
                at_book(&ov, ov1x, ov1y)
            ),
        );

        // o3 - the panel's own backing shows in the GUTTER between two
        // buttons. The buttons are 24 wide on a 25 pitch, so unlike the book's
        // own cells there is a column between them - and it is the panel, at
        // 198, not the page underneath.
        let (gutter_x, _) = ro::button_origin((ox, oy), 0, 2);
        c.record(
            "o3.the_panel_shows_through_the_gutter_between_buttons",
            at_book(&ov, gutter_x + ro::BUTTON_SIZE, ov0y) == [198, 198, 198],
            format!(
                "the column at x+{} reads {:?} - `overlay_recipe`'s interior, which is what a 24-wide button on a 25 pitch leaves",
                ro::BUTTON_SIZE,
                at_book(&ov, gutter_x + ro::BUTTON_SIZE, ov0y)
            ),
        );

        // o4 - hover reads the cursor, and lights exactly one button.
        c.record(
            "o4.hover_lights_the_button_under_the_cursor_and_no_other",
            at_book(&hov, ov0x, ov0y) == [136, 146, 201]
                && at_book(&hov, ov1x, ov1y) == at_book(&ov, ov1x, ov1y),
            format!(
                "the hovered button reads {:?} (`crafting_overlay_highlighted`) while the other is unchanged at {:?}",
                at_book(&hov, ov0x, ov0y),
                at_book(&hov, ov1x, ov1y)
            ),
        );

        // o5 - the FAMILY follows the menu, and it is invisible at the centre.
        //
        // `crafting_overlay` and `furnace_overlay` are byte-identical at
        // (12, 12): both 139. They differ at 96 pixels, all of them the 3x3
        // grid dividers the crafting art draws at x = 8 and x = 15 - so this
        // probes (8, 2), where crafting is 55 and furnace is 139. A witness on
        // the centre would pass with the two families swapped.
        let (dx, dy) = btn(0, 2, 8, 2);
        c.record(
            "o5.the_furnace_family_is_told_apart_where_the_two_sprites_differ",
            at_book(&ov, dx, dy) == [55, 55, 55]
                && at_book(&fur, dx, dy) == [139, 139, 139]
                && at_book(&ov, ov0x, ov0y) == at_book(&fur, ov0x, ov0y),
            format!(
                "at (8, 2) the crafting button reads {:?} and the furnace one {:?}, while their CENTRES are both {:?}",
                at_book(&ov, dx, dy),
                at_book(&fur, dx, dy),
                at_book(&ov, ov0x, ov0y)
            ),
        );

        // o6 - a wider overlay is wider, and the extra buttons are real. Five
        // buttons is the widest a row gets before `maxRow` steps to 5, and it
        // reaches past where two buttons end.
        let (b4x, b4y) = ro::button_origin((ox, oy), 4, 5);
        //
        // Probed at (8, 2) - a crafting DIVIDER, 55 - and not at the centre,
        // because a button's centre is 139 and so is the recipe cell it covers
        // (`slot_craftable` is 139 at the matching texel). The first cut used
        // the centre and could not tell the button from the cell beneath it.
        c.record(
            "o6.a_wider_overlay_draws_the_buttons_a_narrow_one_does_not",
            at_book(&wide, b4x + 8, b4y + 2) == [55, 55, 55]
                && at_book(&ov, b4x + 8, b4y + 2) != [55, 55, 55],
            format!(
                "the fifth button's divider reads {:?} on a five-wide overlay and {:?} on a two-wide one, where that button does not exist",
                at_book(&wide, b4x + 8, b4y + 2),
                at_book(&ov, b4x + 8, b4y + 2)
            ),
        );

        // o7 - and it is drawn LAST. `RecipeBookPage.extractRenderState` calls
        // `graphics.nextStratum()` before the overlay, so it is a layer over
        // the page's ARROWS as well as its cells. Slot 0's cell is covered by
        // o1; this pins that the ordering is not merely "after the cells".
        //
        // The five-wide overlay spans x 15..140 at the top of the grid, which
        // is where a filter toggle and the tabs are not - so the cell row
        // BELOW is the reachable second layer here.
        let (s5x, s5y) = rb::grid_slot(5);
        c.record(
            "o7.the_overlay_is_a_stratum_over_the_page_not_a_peer",
            // Eight buttons is two rows of four, so the second row lands on
            // the cells of page row 1.
            at_book(&deep, s5x + 12, s5y + 12) != at_book(&open, s5x + 12, s5y + 12),
            "an eight-button overlay is two rows deep and its second row covers the page's second row of cells"
                .to_string(),
        );

        // -- the cell's tooltip (M106) --------------------------------------
        //
        // Graded on the LINES rather than on pixels, for the reason o14/o15
        // grade the crafter's hint that way: what a tooltip says is the claim,
        // and the box it is drawn in is `tooltip_layout`'s business, already
        // pinned by the witnesses that share it.
        //
        // Driven through the production `book_tooltip`, not a local
        // reassembly of its parts — M93b's rule.
        let lang = &baked.lang;
        let advance = &baked
            .font
            .as_ref()
            .ok_or("containershot: no baked font")?
            .advance;
        let cell_tip = |b: &crate::live_cmd::BookRender,
                        overlay_open: bool,
                        cell: usize|
         -> Vec<String> {
            let (cx, cy) = rb::grid_slot(cell);
            let mouse = (
                (bl + (cx as f32 + 12.0) * bsc) as f64,
                (bt + (cy as f32 + 12.0) * bsc) as f64,
            );
            crate::live_cmd::book_tooltip(
                b,
                overlay_open,
                Some((cx + 12, cy + 12)),
                &book_items,
                &baked.item_names,
                lang,
                rewo_gpu::tooltip::TooltipFlag::NORMAL,
                advance,
                None,
                mouse,
                (W as f32, H as f32),
            )
            .map(|(_, lines, _)| lines.into_iter().map(|l| l.text).collect())
            .unwrap_or_default()
        };
        let one = book(20, 1, 0);
        let mut many = book(20, 1, 0);
        // `hasMultipleRecipes()` — the second half of the `(craftable,
        // multiple)` pair, and the same flag that picks the `slot_many_*`
        // chrome and lets a right-click open the overlay.
        many.slots = vec![(true, true); rb::ITEMS_PER_PAGE];
        let more = lang.or_key(rb::MORE_RECIPES_KEY).to_string();
        // `Items::name` answers the REGISTRY id, so the display-name map is
        // keyed by `minecraft:dirt` and not `dirt`. Looked up rather than
        // written as "Dirt": the point of the witness is that the tooltip goes
        // through the same translation the menu's does, and a literal would
        // pass against a build that had stopped translating at all.
        let dirt_name = baked
            .item_names
            .get("minecraft:dirt")
            .cloned()
            .ok_or("containershot: no display name for minecraft:dirt")?;

        c.record(
            "b14.a_hovered_cell_names_the_item_it_is_showing",
            cell_tip(&one, false, 3) == vec![dirt_name.clone()],
            format!(
                "hovering cell 3 gives {:?} — the display stack's own name, and nothing else on a single-recipe cell",
                cell_tip(&one, false, 3)
            ),
        );
        c.record(
            "b15.the_more_recipes_line_is_added_only_by_a_MULTI_recipe_cell",
            cell_tip(&many, false, 3) == vec![dirt_name.clone(), more.clone()]
                && cell_tip(&one, false, 3) == vec![dirt_name.clone()],
            format!(
                "multi -> {:?}, single -> {:?}; the string is {more:?} and carries NO count, so a `+N more` line is not what vanilla shows",
                cell_tip(&many, false, 3),
                cell_tip(&one, false, 3)
            ),
        );
        c.record(
            "b16.an_open_overlay_suppresses_the_cells_tooltip",
            cell_tip(&many, true, 3).is_empty() && !cell_tip(&many, false, 3).is_empty(),
            format!(
                "the same hover gives {:?} with the overlay up and {:?} without — the cells still hover underneath it, and vanilla drops the tooltip anyway",
                cell_tip(&many, true, 3),
                cell_tip(&many, false, 3)
            ),
        );
        // Two ways of showing nothing, and they are different states: a cell
        // past the end of the page is not hovered at all, and a cell whose
        // result Rewo cannot resolve is hovered and has nothing to say.
        let mut unresolved = book(20, 1, 0);
        unresolved.slot_items = vec![None; rb::ITEMS_PER_PAGE];
        let short = book(5, 1, 0);
        c.record(
            "b17.an_invisible_or_unresolvable_cell_says_nothing",
            cell_tip(&short, false, 9).is_empty()
                && cell_tip(&unresolved, false, 3).is_empty()
                && !cell_tip(&short, false, 3).is_empty(),
            format!(
                "cell 9 of a five-cell page -> {:?}, an unresolvable result -> {:?}, while cell 3 of the same short page still speaks -> {:?}",
                cell_tip(&short, false, 9),
                cell_tip(&unresolved, false, 3),
                cell_tip(&short, false, 3)
            ),
        );
        // The gutter between the grid and the arrows: a cursor that is in the
        // book but on no cell. Without this, "hovering nothing" is only ever
        // witnessed by an invisible cell, and a build that answered slot 0 for
        // every miss would pass b14-b17.
        c.record(
            "b18.a_cursor_in_the_book_but_on_no_cell_gets_no_tooltip",
            crate::live_cmd::book_tooltip(
                &one,
                false,
                Some((rb::PAGE_LABEL_CENTRE_X, rb::PAGE_LABEL_Y)),
                &book_items,
                &baked.item_names,
                lang,
                rewo_gpu::tooltip::TooltipFlag::NORMAL,
                advance,
                None,
                (0.0, 0.0),
                (W as f32, H as f32),
            )
            .is_none(),
            format!(
                "the page-counter row at ({}, {}) is inside the book and on no widget",
                rb::PAGE_LABEL_CENTRE_X,
                rb::PAGE_LABEL_Y
            ),
        );
    }

    if let Some(d) = &args.out_dir {
        let _ = std::fs::write(d.join("containershot-overlays.txt"), "see the PNGs");
        wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
            menu(11, &[(0, 200), (1, 20)]).open().unwrap(),
            30,
            false,
            effects,
            None,
            None,
            None,
            None,
            None,
        ));
        let _ = shot(gpu, off, wr);
        let _ = off.save_png(gpu, &d.join("containershot-brewing.png"));
    }

    Ok(())
}
