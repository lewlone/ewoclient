//! `rewo animshot --check` — animated block textures, and the frame
//! interpolation 26.2 applies to 23 of them (M195).
//!
//! `SpriteContents.AnimationState.drawToAtlas` either blits the current frame
//! (`ANIMATE_SPRITE_BLIT`) or, when the texture's `.mcmeta` says
//! `"interpolate": true`, draws `mix(current, next, progress)` into the atlas
//! (`animate_sprite_interpolate.fsh`), with `progress =
//! (int)((float)subFrame / time * 1000) / 1000.0`. This gate drives the
//! production path — the bake's `.mcmeta` read, `live_cmd::layer_animations`,
//! `WorldRenderer::anim_tick` and its async upload into the block texture
//! array — and reads the layer BACK from the GPU, comparing it with blends
//! computed here from the jar's own PNG frames and `.mcmeta` files (never
//! from the bake under test).
//!
//! **Fail-closed** on [`EXPECTED_WITNESSES`].

use std::path::{Path, PathBuf};

use clap::Args as ClapArgs;

use rewo_data::{assets, DataPaths};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

/// a1-a9.
const EXPECTED_WITNESSES: usize = 9;

#[derive(ClapArgs)]
pub struct AnimshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Assert the properties and exit nonzero on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Bypass Vulkan validation layers.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[animshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn jar_bytes(jar: &Path, path: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(jar).ok()?;
    let mut z = zip::ZipArchive::new(f).ok()?;
    let mut bytes = Vec::new();
    z.by_name(path).ok()?.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// A texture's 16 x 16 frames, top to bottom, straight from the jar's PNG.
fn jar_frames(jar: &Path, texture: &str) -> Option<Vec<Vec<u8>>> {
    let bytes = jar_bytes(jar, &format!("assets/minecraft/textures/{texture}.png"))?;
    let (rgba, w, h) = crate::skin_fetch::decode_png_rgba(&bytes, (16, 4096)).ok()?;
    (w == 16 && h % 16 == 0).then(|| rgba.chunks(16 * 16 * 4).map(<[u8]>::to_vec).collect())
}

/// `(frametime, frames, interpolate)` from the jar's `.mcmeta`.
fn jar_mcmeta(jar: &Path, texture: &str) -> Option<(u32, Option<Vec<u32>>, bool)> {
    let bytes = jar_bytes(
        jar,
        &format!("assets/minecraft/textures/{texture}.png.mcmeta"),
    )?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let a = v.get("animation")?;
    let time = a.get("frametime").and_then(|t| t.as_u64()).unwrap_or(1) as u32;
    let frames = a.get("frames").and_then(|f| f.as_array()).map(|l| {
        l.iter()
            .filter_map(|x| x.as_u64().map(|i| i as u32))
            .collect()
    });
    let interpolate = a
        .get("interpolate")
        .and_then(|i| i.as_bool())
        .unwrap_or(false);
    Some((time, frames, interpolate))
}

/// The shader, transcribed again: UNORM samples, `mix`, a round-to-nearest
/// store.
fn mix_bytes(a: &[u8], b: &[u8], milli: u32) -> Vec<u8> {
    let p = milli as f32 / 1000.0;
    a.iter()
        .zip(b)
        .map(|(&x, &y)| {
            let v = (f32::from(x) / 255.0) * (1.0 - p) + (f32::from(y) / 255.0) * p;
            (v * 255.0).round() as u8
        })
        .collect()
}

/// A plain 2 x 2 box average of a 16 x 16 frame, to 8 x 8.
fn box_half(f: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 8 * 8 * 4];
    for y in 0..8 {
        for x in 0..8 {
            for c in 0..4 {
                let s: u32 = [(0, 0), (0, 1), (1, 0), (1, 1)]
                    .iter()
                    .map(|(dy, dx)| u32::from(f[((y * 2 + dy) * 16 + x * 2 + dx) * 4 + c]))
                    .sum();
                out[(y * 8 + x) * 4 + c] = (s / 4) as u8;
            }
        }
    }
    out
}

/// The largest per-byte difference.
fn max_diff(a: &[u8], b: &[u8]) -> u8 {
    if a.len() != b.len() {
        return u8::MAX;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

pub fn run(args: AnimshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    let name_of = |layer: u16| -> Option<String> {
        let n = baked.layer_names.get(layer as usize)?;
        let n = n.split('#').next()?;
        Some(n.strip_prefix("minecraft:").unwrap_or(n).to_string())
    };

    // a1: every baked animation carries its jar `.mcmeta`'s `interpolate`.
    let mut agree = 0;
    let mut interpolating = 0;
    let mut wrong = Vec::new();
    for a in &baked.animations {
        let Some(name) = name_of(a.layer) else {
            wrong.push(format!("layer {} unnamed", a.layer));
            continue;
        };
        let want = jar_mcmeta(&jar, &name).map(|m| m.2);
        if want == Some(a.interpolate) {
            agree += 1;
            interpolating += usize::from(a.interpolate);
        } else {
            wrong.push(format!("{name}: baked {} jar {want:?}", a.interpolate));
        }
    }
    c.record(
        "a1.the_bake_carries_each_mcmetas_interpolate",
        agree == baked.animations.len() && interpolating >= 10,
        format!(
            "{agree} of {} animated layers agree with their .mcmeta, {interpolating} \
             interpolate; disagreements {wrong:?}",
            baked.animations.len()
        ),
    );

    let find = |texture: &str| {
        baked
            .animations
            .iter()
            .find(|a| name_of(a.layer).as_deref() == Some(texture))
            .map(|a| a.layer)
    };
    let magma = find("block/magma").ok_or("magma is not an animated layer")?;
    let lava = find("block/lava_still").ok_or("lava_still is not an animated layer")?;
    let mf = jar_frames(&jar, "block/magma").ok_or("magma.png")?;
    let (mtime, _, _) = jar_mcmeta(&jar, "block/magma").ok_or("magma.png.mcmeta")?;
    let lf = jar_frames(&jar, "block/lava_still").ok_or("lava_still.png")?;
    let (ltime, lorder, _) = jar_mcmeta(&jar, "block/lava_still").ok_or("lava mcmeta")?;
    let lorder = lorder.unwrap_or_else(|| (0..lf.len() as u32).collect());

    let mut gpu = Gpu::new(None, !args.no_validation)?;
    println!(
        "[animshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !args.no_validation && !gpu.validation_active {
        return Err("animshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, 16, 16)?;
    let mut wr = WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    crate::live_cmd::install_animations(&mut wr, &mut gpu, &baked)?;
    let mut at = |gpu: &mut Gpu, tick: u64, layer: u16, level: u32| -> Result<Vec<u8>, String> {
        wr.anim_tick(gpu, tick)?;
        wr.read_texture_layer(gpu, layer, level)
    };

    // a2: tick 0 is frame 0 exactly — progress 0 is `mix(c, n, 0)`.
    let t0 = at(&mut gpu, 0, magma, 0)?;
    c.record(
        "a2.tick_0_is_the_first_frame",
        max_diff(&t0, &mf[0]) == 0,
        format!(
            "max byte difference {} from magma frame 0",
            max_diff(&t0, &mf[0])
        ),
    );

    // a3: half way through frame 0 (subFrame 4 of 8) the layer is the blend,
    // and the blend is visible (the frames differ at many bytes).
    let half = mtime / 2;
    let t4 = at(&mut gpu, u64::from(half), magma, 0)?;
    let want4 = mix_bytes(&mf[0], &mf[1], half * 1000 / mtime);
    let moved = mf[0]
        .iter()
        .zip(&mf[1])
        .filter(|(a, b)| a.abs_diff(**b) > 8)
        .count();
    c.record(
        "a3.half_way_through_a_frame_is_the_blend",
        max_diff(&t4, &want4) <= 1 && max_diff(&t4, &mf[0]) > 8 && moved > 100,
        format!(
            "tick {half}: within {} of mix(f0, f1, 0.5) (want <= 1), {} from frame 0, the \
             frames differ at {moved} bytes",
            max_diff(&t4, &want4),
            max_diff(&t4, &mf[0])
        ),
    );

    // a4: the frame boundary is the next frame exactly.
    let t8 = at(&mut gpu, u64::from(mtime), magma, 0)?;
    c.record(
        "a4.the_next_frame_starts_unblended",
        max_diff(&t8, &mf[1]) == 0,
        format!(
            "tick {mtime}: max difference {} from frame 1",
            max_diff(&t8, &mf[1])
        ),
    );

    // a5: the last frame blends back into the first (`(frame + 1) % size`),
    // at subFrame 7 of 8: progress `(int)(0.875 * 1000)`.
    let last = mf.len() as u32 - 1;
    let tick = u64::from(last * mtime + mtime - 1);
    let tl = at(&mut gpu, tick, magma, 0)?;
    let milli = ((mtime - 1) as f32 / mtime as f32 * 1000.0) as u32;
    let wantl = mix_bytes(&mf[last as usize], &mf[0], milli);
    c.record(
        "a5.the_last_frame_blends_into_the_first",
        max_diff(&tl, &wantl) <= 1 && max_diff(&tl, &mf[last as usize]) > 8,
        format!(
            "tick {tick}: within {} of mix(f{last}, f0, {milli}/1000)",
            max_diff(&tl, &wantl)
        ),
    );

    // a6: lava does NOT interpolate: mid-frame it is its frame, unblended,
    // and the next step is the next frame of its ping-pong order.
    let l1 = at(&mut gpu, 1, lava, 0)?;
    let l2 = at(&mut gpu, u64::from(ltime), lava, 0)?;
    let f0 = &lf[lorder[0] as usize];
    let f1 = &lf[lorder[1 % lorder.len()] as usize];
    c.record(
        "a6.a_texture_without_interpolate_cuts",
        max_diff(&l1, f0) == 0 && max_diff(&l2, f1) == 0 && max_diff(f0, f1) > 8,
        format!(
            "lava tick 1: {} from order[0]; tick {ltime}: {} from order[1]",
            max_diff(&l1, f0),
            max_diff(&l2, f1)
        ),
    );

    // a7: the mips blend too (vanilla mixes each mip level of the two
    // frames): level 1 half way through is the blend of the two frames'
    // halves, to within the box filter's rounding.
    let m1 = at(&mut gpu, u64::from(half), magma, 1)?;
    let wantm = mix_bytes(&box_half(&mf[0]), &box_half(&mf[1]), half * 1000 / mtime);
    c.record(
        "a7.the_mip_levels_blend_as_well",
        max_diff(&m1, &wantm) <= 2 && max_diff(&m1, &box_half(&mf[0])) > 8,
        format!(
            "level 1 within {} of the blended halves (want <= 2)",
            max_diff(&m1, &wantm)
        ),
    );

    // a8 (M196): a lava SIDE samples `lava_flow`'s top-left quadrant, so the
    // side layer is that quadrant of each 32 x 32 frame, stepping on the flow
    // texture's own `.mcmeta` clock (frametime 3, sequential).
    let side = baked.render.iter().find_map(|r| match r {
        rewo_data::assets::RenderKind::Fluid {
            side_layer,
            lava: true,
            ..
        } => Some(*side_layer),
        _ => None,
    });
    let flow = jar_bytes(&jar, "assets/minecraft/textures/block/lava_flow.png")
        .and_then(|b| crate::skin_fetch::decode_png_rgba(&b, (32, 4096)).ok());
    let (ftime, _, _) = jar_mcmeta(&jar, "block/lava_flow").ok_or("lava_flow mcmeta")?;
    let quadrant = |rgba: &[u8], frame: usize| -> Vec<u8> {
        (0..16)
            .flat_map(|y| {
                let row = (frame * 32 + y) * 32 * 4;
                rgba[row..row + 16 * 4].to_vec()
            })
            .collect()
    };
    let (ok8, detail8) = match (side, flow) {
        (Some(side), Some((rgba, 32, _))) => {
            let s0 = at(&mut gpu, 0, side, 0)?;
            let s1 = at(&mut gpu, u64::from(ftime), side, 0)?;
            let (q0, q1) = (quadrant(&rgba, 0), quadrant(&rgba, 1));
            (
                max_diff(&s0, &q0) == 0 && max_diff(&s1, &q1) == 0 && max_diff(&q0, &q1) > 8,
                format!(
                    "side layer {side}: tick 0 {} from frame 0's quadrant, tick {ftime} {} from frame 1's",
                    max_diff(&s0, &q0),
                    max_diff(&s1, &q1)
                ),
            )
        }
        (side, flow) => (
            false,
            format!(
                "no lava side layer ({side:?}) or no 32-wide lava_flow.png ({:?})",
                flow.map(|f| (f.1, f.2))
            ),
        ),
    };
    c.record("a8.a_lava_side_animates_on_the_flow_quadrant", ok8, detail8);

    // a9 (M197): a flowing lava TOP samples the whole `lava_flow` sprite, so
    // the flow array's lava layer is each 32 x 32 frame entire, on the same
    // clock — read back from the flow array `install_animations` built.
    let top = baked.render.iter().find_map(|r| match r {
        rewo_data::assets::RenderKind::Fluid {
            flow_layer,
            lava: true,
            ..
        } => Some(*flow_layer),
        _ => None,
    });
    let flow = jar_bytes(&jar, "assets/minecraft/textures/block/lava_flow.png")
        .and_then(|b| crate::skin_fetch::decode_png_rgba(&b, (32, 4096)).ok());
    let frame_of = |rgba: &[u8], f: usize| rgba[f * 32 * 32 * 4..(f + 1) * 32 * 32 * 4].to_vec();
    let (ok9, detail9) = match (top, flow) {
        (Some(top), Some((rgba, 32, _))) if top != rewo_data::assets::NO_FLOW_LAYER => {
            wr.anim_tick(&mut gpu, 0)?;
            let t0 = wr.read_flow_texture_layer(&mut gpu, top, 0)?;
            wr.anim_tick(&mut gpu, u64::from(ftime))?;
            let t1 = wr.read_flow_texture_layer(&mut gpu, top, 0)?;
            let (f0, f1) = (frame_of(&rgba, 0), frame_of(&rgba, 1));
            (
                max_diff(&t0, &f0) == 0 && max_diff(&t1, &f1) == 0 && max_diff(&f0, &f1) > 8,
                format!(
                    "flow layer {top}: tick 0 {} from frame 0, tick {ftime} {} from frame 1 ({} bytes, the whole 32 x 32 frame)",
                    max_diff(&t0, &f0),
                    max_diff(&t1, &f1),
                    t0.len()
                ),
            )
        }
        (top, flow) => (
            false,
            format!(
                "no lava flow layer ({top:?}) or no 32-wide lava_flow.png ({:?})",
                flow.map(|f| (f.1, f.2))
            ),
        ),
    };
    c.record(
        "a9.a_flowing_lava_top_samples_the_whole_flow_frame",
        ok9,
        detail9,
    );

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);

    println!(
        "[animshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "animshot: {} witnesses failed: {}",
            c.failures.len(),
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "animshot: witness count {} != expected {EXPECTED_WITNESSES} (fail-closed)",
            c.witnessed
        ));
    }
    println!("[animshot] PASS — {} witnesses", c.witnessed);
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
