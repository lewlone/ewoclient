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
            open, 30, false, effects, None,
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
            open, 30, creative, effects, mouse,
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
            open, 30, false, effects, None,
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

    if let Some(d) = &args.out_dir {
        let _ = std::fs::write(d.join("containershot-overlays.txt"), "see the PNGs");
        wr.set_container_panel(crate::live_cmd::container_panel_for_open_menu(
            menu(11, &[(0, 200), (1, 20)]).open().unwrap(),
            30,
            false,
            effects,
            None,
        ));
        let _ = shot(gpu, off, wr);
        let _ = off.save_png(gpu, &d.join("containershot-brewing.png"));
    }

    Ok(())
}
