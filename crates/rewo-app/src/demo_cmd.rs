//! `rewo demo` — M4 headless showcase. Builds a synthetic in-memory world
//! (grass platform + a row of varied blocks) and renders it to a PNG. No
//! server needed — this is the deterministic artifact that proves the model
//! path, AO, and tint, showing block families a flat world never contains.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::{assets, DataPaths, GameData};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::Gpu;
use rewo_gpu::world::WorldRenderer;
use rewo_world::dimension::DimensionShape;
use rewo_world::World;

use crate::stats::OverlayRing;

const CLEAR_SKY: [f32; 4] = [0.184, 0.380, 1.0, 1.0];

#[derive(ClapArgs)]
pub struct DemoArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    #[arg(long, default_value = "rewo-m4-demo.png")]
    out: PathBuf,
    /// Render at this texture-animation game tick (water/lava frames) —
    /// two PNGs at different ticks must differ only in fluid texels.
    #[arg(long, default_value_t = 0)]
    anim_tick: u64,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

/// Blocks to showcase — each exercises a different model family.
const SHOWCASE: &[&str] = &[
    "minecraft:cobblestone",   // plain cube
    "minecraft:oak_slab",      // half box + cullfaces
    "minecraft:oak_stairs",    // multi-box + variant rotation
    "minecraft:oak_fence",     // multipart (post only, no connections)
    "minecraft:glass",         // cutout cube
    "minecraft:oak_log",       // pillar (axis)
    "minecraft:torch",         // thin box, no shade
    "minecraft:poppy",         // cross plant (45° rescale)
    "minecraft:dandelion",     // cross plant
    "minecraft:crafting_table",// directional side textures
];

pub fn run(args: DemoArgs) -> Result<(), String> {
    let data = GameData::load_for_version(&args.version)?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    log::info!(
        "demo: baked {} cubes + {} models",
        baked.stats.cube_states,
        baked.stats.model_states
    );

    // -- build the synthetic scene ----------------------------------------
    let mut world = World::new(DimensionShape::OVERWORLD);
    let grass = data.blocks.default_state("minecraft:grass_block").unwrap_or(9);
    let dirt = data.blocks.default_state("minecraft:dirt").unwrap_or(10);
    // A grass-over-dirt platform: 3 rows behind the showcase line plus a
    // 4-row apron in front (z −4..−1) that hosts the fluid pools.
    let n = SHOWCASE.len() as i32;
    for x in -1..=n {
        for z in -4..=1 {
            ensure_and_set(&mut world, x, -1, z, dirt);
            ensure_and_set(&mut world, x, 0, z, grass);
        }
    }
    // Fluid pools carved into the apron (M4-followon: fluids). Water left,
    // lava right, a grass spine at x=5 between them. One flowing (level 4)
    // water cell shows the sloped-surface path; water states are id-ordered
    // by the `level` property, so default (source) + 4 = level 4.
    let water = data.blocks.default_state("minecraft:water").unwrap_or(0);
    let lava = data.blocks.default_state("minecraft:lava").unwrap_or(0);
    for z in -3..=-1 {
        for x in 2..=4 {
            ensure_and_set(&mut world, x, 0, z, water);
        }
        for x in 6..=8 {
            ensure_and_set(&mut world, x, 0, z, lava);
        }
    }
    ensure_and_set(&mut world, 4, 0, -3, water + 4);
    // Row of showcase blocks on top of the grass (y=1), spaced 1 apart.
    let mut placed = Vec::new();
    for (i, name) in SHOWCASE.iter().enumerate() {
        let x = i as i32;
        match data.blocks.default_state(name) {
            Some(state) => {
                ensure_and_set(&mut world, x, 1, 0, state);
                // Pillar for the log so its axis reads.
                if *name == "minecraft:oak_log" {
                    ensure_and_set(&mut world, x, 2, 0, state);
                }
                placed.push((*name, kind_of(&baked, state)));
            }
            None => log::warn!("demo: {name} not in block table"),
        }
    }
    log::info!("demo: placed {} showcase blocks", placed.len());
    for (name, kind) in &placed {
        log::info!("demo:   {name} → {kind}");
    }

    // -- mesh --------------------------------------------------------------
    let mut gpu = Gpu::new(None, cfg!(debug_assertions) && !args.no_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut wr = WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    let mut total_verts = 0;
    for (cx, cz) in world.column_coords() {
        if let Some(mesh) = rewo_mesh::mesh_column(&world, &baked.render, &baked.models, cx, cz) {
            total_verts += mesh.vertices.len();
            wr.upload_column(
                &mut gpu,
                cx,
                cz,
                bytemuck::cast_slice(&mesh.vertices),
                &mesh.indices,
                bytemuck::cast_slice(&mesh.tvertices),
                &mesh.tindices,
                mesh.y_min,
                mesh.y_max,
            )?;
        }
    }
    gpu.wait_idle();
    log::info!("demo: meshed {} vertices", total_verts);

    wr.set_animations(crate::live_cmd::layer_animations(&baked));
    wr.anim_tick(&mut gpu, args.anim_tick)?;

    // -- camera: look along the row from a low front-left angle (aim point
    // dropped to y=1.0 so the fluid apron in front reads too) -------------
    let center = Vec3::new(n as f32 / 2.0, 1.0, 0.5);
    let eye = Vec3::new(n as f32 / 2.0 - 2.0, 3.7, -4.5);
    wr.set_camera(eye.to_array());
    let dir = (center - eye).normalize();
    let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        55f32.to_radians(),
        1280.0 / 720.0,
        0.05,
    ));
    let vp = (proj * view).to_cols_array_2d();

    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [16.0, 16.0],
        size: [560.0, 140.0],
    };
    for _ in 0..3 {
        off.render(&gpu, Some((&mut wr, vp)), &draw, CLEAR_SKY)?;
    }
    off.save_png(&gpu, &args.out)?;
    println!(
        "[rewo-m4] demo: {} blocks, {} vertices, {} drawn, wrote {}",
        placed.len(),
        total_verts,
        wr.drawn_last_frame,
        args.out.display()
    );
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

fn ensure_and_set(world: &mut World, x: i32, y: i32, z: i32, state: u32) {
    world.ensure_column(x >> 4, z >> 4);
    world.set_block(x, y, z, state);
}

fn kind_of(baked: &assets::BakedAssets, state: u32) -> &'static str {
    match baked.render.get(state as usize) {
        Some(assets::RenderKind::Cube { .. }) => "cube",
        Some(assets::RenderKind::Model(_)) => "model",
        Some(assets::RenderKind::Fluid { .. }) => "fluid",
        _ => "invisible",
    }
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}
