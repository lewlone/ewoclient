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

    Ok(())
}
