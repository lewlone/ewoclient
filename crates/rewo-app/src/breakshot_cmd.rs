//! `rewo breakshot --check` — M81's block-break crack oracle.
//!
//! `ClientboundBlockDestructionPacket` was falling off the dispatch chain, so
//! a block somebody else was mining never showed a crack. This gate drives the
//! whole chain:
//!
//! ```text
//! raw block_destruction body (VarInt id + packed BlockPos + unsigned byte)
//!   -> rewo_net::route_block_destruction    (production packet-id selection)
//!   -> ClientLevel.destroyBlockProgress     (the two indexes + the range test)
//!   -> rewo_mesh::crumbling                 (the block's own quads, decal UVs)
//!   -> the CRUMBLING pipeline, read back    (multiply blend, in gamma space)
//! ```
//!
//! # What the pixel half measures, and what it deliberately does not
//!
//! The blend is `2·src·dst` — a **multiply** — so the crack's job is to make
//! the block darker, and the property to measure is exactly that: a cracked
//! face is darker than the same face uncracked, more so at a later stage, and
//! **byte-identical** to it when no record exists. A count of "how many pixels
//! are dark" would be a proxy; the darkening ratio is the thing itself.
//!
//! The one number that is predicted rather than merely compared is the gamma
//! space the multiply runs in: `2·src·dst` evaluated on linear values and then
//! sRGB-encoded is materially different from the same multiply on the encoded
//! bytes, and a witness pins which one the GPU did — the M50 finding, in its
//! second place.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_gpu::crumbling::CrumblingVertex;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_mesh::MeshVertex;
use rewo_net::ids::Ids;
use rewo_net::route_block_destruction;
use rewo_world::destruction::DestructionProgress;

use crate::stats::OverlayRing;

/// `a1`-`a6` receipt + store, `b1`-`b8` geometry, `c1`-`c8` pixels.
const EXPECTED_WITNESSES: usize = 22;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const TEX: u32 = 16;
const W: u32 = 128;
const H: u32 = 128;

#[derive(ClapArgs)]
pub struct BreakshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally.
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Optional: dump the rendered frames here for eyeballing a failure.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
    /// Bypass Vulkan validation layers. Otherwise the gate requests them in
    /// every build and, under `--check`, fails if they don't activate.
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
            "[breakshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: BreakshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!("[breakshot] mode: {mode} (the oracle asserts unconditionally)");

    let paths = rewo_data::DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = rewo_data::packets::Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_receipt(&mut c, &ids);
    check_geometry(&mut c);

    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[breakshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err("breakshot check: Vulkan validation requested but not active — \
                    install the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass \
                    --no-validation to bypass"
            .into());
    }
    let pixels = check_pixels(&mut c, &mut gpu, &args);

    println!(
        "[breakshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
    );
    pixels?;
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
    println!("[breakshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ------------------------------------------------------------------- bodies

fn varint(mut v: i32, out: &mut Vec<u8>) {
    let mut u = v as u32;
    loop {
        let b = (u & 0x7F) as u8;
        u >>= 7;
        v = u as i32;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// `ClientboundBlockDestructionPacket` — VarInt id, packed `BlockPos`,
/// **unsigned** byte.
///
/// The position packing is vanilla's `BlockPos.asLong`: x in the top 26 bits,
/// z in the next 26, y in the low 12.
fn destruction_body(id: i32, pos: [i32; 3], progress: u8) -> Vec<u8> {
    let mut b = Vec::new();
    varint(id, &mut b);
    let packed = ((pos[0] as i64 & 0x3FF_FFFF) << 38)
        | ((pos[2] as i64 & 0x3FF_FFFF) << 12)
        | (pos[1] as i64 & 0xFFF);
    b.extend_from_slice(&packed.to_be_bytes());
    b.push(progress);
    b
}

// ------------------------------------------------------------------ receipt

fn check_receipt(c: &mut Checker, ids: &Ids) {
    c.record(
        "a1.the_block_destruction_id_resolves_and_is_distinct",
        ids.cb_play_block_destruction != ids.cb_play_block_update
            && ids.cb_play_block_destruction != ids.cb_play_block_event,
        format!(
            "block_destruction={} vs block_update={} vs block_event={}",
            ids.cb_play_block_destruction, ids.cb_play_block_update, ids.cb_play_block_event
        ),
    );

    let mut d = DestructionProgress::default();
    let routed = route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [10, 64, -3], 4),
        ids,
        &mut d,
        0,
    );
    c.record(
        "a2.a_stage_in_range_reaches_the_store_at_the_decoded_position",
        routed && d.stage_at([10, 64, -3]) == Some(4),
        format!(
            "routed={routed}; stage at (10,64,-3) = {:?} (want 4 — and a negative z \
             proves the 26-bit sign extension)",
            d.stage_at([10, 64, -3])
        ),
    );

    // **Any value outside 0..10 retires the record**, not only the server's
    // stop signal.
    //
    // The wire byte is `readUnsignedByte`, so the server's `(byte) -1` arrives
    // as 255 — but a mutation battery showed that reading it *signed* is
    // behaviourally identical here, because −1 and −56 are both outside the
    // range just as 255 and 200 are. The signedness is genuinely unobservable
    // through this test, so this witness does not claim it. What it does claim
    // is the shape of the test:
    //
    // MUTATION: keying the removal on a **sentinel** (`progress == 255`, the
    // reading a "-1 means stop" description invites) keeps a record for 200,
    // whose texture does not exist.
    let mut d = DestructionProgress::default();
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [0, 0, 0], 3),
        ids,
        &mut d,
        0,
    );
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [0, 0, 0], 0xFF),
        ids,
        &mut d,
        1,
    );
    let after_ff = d.breaker_count();
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(6, [0, 0, 0], 3),
        ids,
        &mut d,
        1,
    );
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(6, [0, 0, 0], 200),
        ids,
        &mut d,
        2,
    );
    c.record(
        "a3.any_out_of_range_stage_retires_the_record_not_just_the_stop_signal",
        after_ff == 0 && d.breaker_count() == 0,
        format!(
            "0xFF (the server's `(byte) -1`) retires: {after_ff} breakers left; \
             200 retires too: {} — a sentinel test would keep the second",
            d.breaker_count()
        ),
    );

    // Ten is out of range: the stages are 0..=9 and `DESTROY_STAGE_COUNT` is
    // the exclusive bound.
    let mut d = DestructionProgress::default();
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [0, 0, 0], 9),
        ids,
        &mut d,
        0,
    );
    let nine = d.stage_at([0, 0, 0]);
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [0, 0, 0], 10),
        ids,
        &mut d,
        1,
    );
    c.record(
        "a4.stage_ten_is_a_removal_not_a_final_frame",
        nine == Some(9) && d.stage_at([0, 0, 0]).is_none(),
        format!(
            "stage 9 → {nine:?}, stage 10 → {:?} (a `<= 10` bound would keep a record \
             whose texture does not exist)",
            d.stage_at([0, 0, 0])
        ),
    );

    // One breaker, one block. Moving on retires the old crack — the reason
    // the store is keyed by breaker id at all.
    let mut d = DestructionProgress::default();
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [0, 0, 0], 5),
        ids,
        &mut d,
        0,
    );
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(5, [1, 0, 0], 1),
        ids,
        &mut d,
        1,
    );
    c.record(
        "a5.one_breaker_cracks_one_block",
        d.stage_at([0, 0, 0]).is_none() && d.stage_at([1, 0, 0]) == Some(1) && d.position_count() == 1,
        format!(
            "old block {:?}, new block {:?}, {} positions",
            d.stage_at([0, 0, 0]),
            d.stage_at([1, 0, 0]),
            d.position_count()
        ),
    );

    // Two breakers on one block: `progresses.last()` on a set ordered by
    // `(progress, id)` picks the **furthest along**.
    //
    // MUTATION: `first()` picks the least, and "most recent" picks breaker 2.
    let mut d = DestructionProgress::default();
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(1, [0, 0, 0], 8),
        ids,
        &mut d,
        0,
    );
    route_block_destruction(
        ids.cb_play_block_destruction,
        &destruction_body(2, [0, 0, 0], 2),
        ids,
        &mut d,
        0,
    );
    c.record(
        "a6.the_furthest_along_breaker_wins_a_shared_block",
        d.stage_at([0, 0, 0]) == Some(8) && d.breaker_count() == 2,
        format!(
            "two breakers at stages 8 and 2 → {:?} ({} records) — not the first to \
             arrive and not the last",
            d.stage_at([0, 0, 0]),
            d.breaker_count()
        ),
    );
}

// ----------------------------------------------------------------- geometry

/// A `RenderKind::Cube` table and an empty model list — enough for the decal
/// builder, which reads geometry and never a texture.
fn cube_table() -> Vec<rewo_data::assets::RenderKind> {
    vec![rewo_data::assets::RenderKind::Cube {
        faces: [0; 6],
        raw_faces: [0; 6],
        tint: [rewo_data::assets::TintSource::None; 6],
    }]
}

fn check_geometry(c: &mut Checker) {
    use rewo_mesh::crumbling::{block_decal_quads, decal_uv};

    let quads = block_decal_quads(&cube_table(), &[], 0, [3, 5, 7]);
    c.record(
        "b1.a_cube_emits_every_face_with_no_cullface_test",
        quads.len() == 6,
        format!(
            "{} quads (vanilla walks `part.getQuads(d)` for every direction plus the \
             null list, and relies on the depth test to hide the buried ones)",
            quads.len()
        ),
    );
    let inside = quads.iter().all(|q| {
        q.verts.iter().all(|v| {
            (3.0..=4.0).contains(&v[0]) && (5.0..=6.0).contains(&v[1]) && (7.0..=8.0).contains(&v[2])
        })
    });
    c.record(
        "b2.the_decal_is_in_world_space_at_the_block",
        inside,
        "every corner lies inside the block's own unit cube at (3,5,7)",
    );

    // The UV is regenerated from the *position*, not taken from the model —
    // `SheetedDecalTextureGenerator.setUv` is a no-op.
    //
    // MUTATION: passing the model's atlas UV through would make the top face's
    // coordinate depend on which texture the block uses, not on x/z.
    let top = decal_uv([0.25, 1.0, 0.75], 0);
    c.record(
        "b3.the_top_faces_coordinate_is_the_bare_xz_projection",
        (top[0] - 0.25).abs() < 1e-5 && (top[1] - 0.75).abs() < 1e-5,
        format!("(0.25, y, 0.75) on the top face → {top:?} — the one face whose rotation chain is the identity"),
    );

    // Every face maps to a whole tile, which is what makes a standalone 16×16
    // `destroy_stage` texture cover a block face exactly once.
    let mut spans = Vec::new();
    for face in 0..6 {
        let corners = [
            block_decal_quads(&cube_table(), &[], 0, [0, 0, 0])[face].uv[0],
            block_decal_quads(&cube_table(), &[], 0, [0, 0, 0])[face].uv[1],
            block_decal_quads(&cube_table(), &[], 0, [0, 0, 0])[face].uv[2],
            block_decal_quads(&cube_table(), &[], 0, [0, 0, 0])[face].uv[3],
        ];
        let us: Vec<f32> = corners.iter().map(|q| q[0]).collect();
        let vs: Vec<f32> = corners.iter().map(|q| q[1]).collect();
        let span = |a: &[f32]| {
            a.iter().cloned().fold(f32::MIN, f32::max) - a.iter().cloned().fold(f32::MAX, f32::min)
        };
        spans.push((span(&us), span(&vs)));
    }
    c.record(
        "b4.every_face_maps_to_exactly_one_tile",
        spans
            .iter()
            .all(|(u, v)| (u - 1.0).abs() < 1e-4 && (v - 1.0).abs() < 1e-4),
        format!("per-face (u span, v span): {spans:?}"),
    );

    // The projection is planar — to a tolerance, because `sin(π)` is not zero
    // in f32 and JOML's `rotateY(PI)` leaks the same ~1e-7 cross-term vanilla
    // does. A bit-exact assertion would assert more than the original.
    let (a, b) = (decal_uv([0.3, 0.0, 0.7], 0), decal_uv([0.3, 1.0, 0.7], 0));
    c.record(
        "b5.the_projection_does_not_vary_along_its_own_normal",
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5,
        format!("{a:?} vs {b:?} across a whole block of movement in y"),
    );

    // Four of the six faces come out negative, which is why the sampler must
    // REPEAT: clamping would smear one edge texel across them.
    let negatives = (0..6)
        .filter(|&f| {
            let q = block_decal_quads(&cube_table(), &[], 0, [0, 0, 0])[f];
            q.uv.iter().any(|uv| uv[0] < -1e-4 || uv[1] < -1e-4)
        })
        .count();
    c.record(
        "b6.the_regenerated_coordinate_is_signed",
        negatives > 0,
        format!(
            "{negatives} of 6 faces produce a negative coordinate — CLAMP_TO_EDGE \
             would smear their crack into a single edge texel"
        ),
    );

    // A state with no model geometry cracks nothing: vanilla's guard is
    // `getRenderShape() == RenderShape.MODEL`.
    let invisible = block_decal_quads(&[rewo_data::assets::RenderKind::Invisible], &[], 0, [0; 3]);
    let fluid = block_decal_quads(
        &[rewo_data::assets::RenderKind::Fluid {
            layer: 0,
            raw_layer: 0,
            level: 0,
            lava: false,
        }],
        &[],
        0,
        [0; 3],
    );
    c.record(
        "b7.a_block_with_no_model_cracks_nothing",
        invisible.is_empty() && fluid.is_empty(),
        format!(
            "invisible → {} quads, fluid → {} quads",
            invisible.len(),
            fluid.len()
        ),
    );
    let unknown = block_decal_quads(&[], &[], 9999, [0; 3]);
    c.record(
        "b8.an_unknown_block_state_cracks_nothing",
        unknown.is_empty(),
        "an out-of-range state degrades to no geometry rather than panicking",
    );
}

// ------------------------------------------------------------------- pixels

/// The block texture's grey, and the decal greys per stage.
const BLOCK_GREY: u8 = 160;
/// Stage 0 is deliberately **above** mid-grey: `2·src·dst` brightens there,
/// which is the property `c8` pins.
const LIGHT_STAGE: u8 = 200;
/// Stages 1..=9 descend from here, all below mid-grey, so they darken.
fn dark_stage(i: usize) -> u8 {
    130 - (i as u8) * 10
}

/// The crumbling stage textures the pixel half uses: **uniform grey**, one per
/// stage.
///
/// Synthetic rather than the jar's own art, and that is the point. A real
/// `destroy_stage` texture is mostly transparent with a few dark lines, so a
/// patch of it is dominated by the alpha cut and the measured result depends
/// on which texel the sample landed on. A uniform texture makes the multiply
/// the *only* variable, so the witness measures the blend rather than the
/// artwork.
fn grey_stages() -> Vec<Vec<u8>> {
    (0..10)
        .map(|i| {
            let v = if i == 0 { LIGHT_STAGE } else { dark_stage(i) };
            let mut px = Vec::with_capacity((TEX * TEX * 4) as usize);
            for _ in 0..(TEX * TEX) {
                px.extend_from_slice(&[v, v, v, 255]);
            }
            px
        })
        .collect()
}

/// The **top face** of the block at `[0, 64, 0]`, in the world pass's own
/// vertex format, fullbright and untinted.
///
/// At `y = 65`, i.e. exactly where the decal's up-face lands — and that is
/// load-bearing, not incidental. A first draft put this at `y = 64`, a whole
/// block below the decal, and the pixel witnesses then passed with a strict
/// `GREATER` depth test because the decal was floating in front rather than
/// coplanar. The coplanar case is the entire reason vanilla's pipeline uses
/// `GREATER_THAN_OR_EQUAL`, so the gate has to be in it.
fn floor_quad() -> (Vec<MeshVertex>, Vec<u32>) {
    let y = 65.0f32;
    let v = |x: f32, z: f32, u: f32, w: f32| {
        MeshVertex::new([x, y, z], [u, w], 0, 15, 15, 0, 3, [255, 255, 255])
    };
    (
        vec![
            v(0.0, 0.0, 0.0, 0.0),
            v(1.0, 0.0, 1.0, 0.0),
            v(1.0, 1.0, 1.0, 1.0),
            v(0.0, 1.0, 0.0, 1.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

/// The top face of the block at the origin, as decal geometry at `stage`.
fn decal_verts(stage: u32) -> Vec<CrumblingVertex> {
    let q = rewo_mesh::crumbling::block_decal_quads(&cube_table(), &[], 0, [0, 64, 0]);
    // Face 0 is up (+Y) — the one the camera looks straight down at.
    let q = q[0];
    let v = |i: usize| CrumblingVertex {
        pos: q.verts[i],
        uv: q.uv[i],
        stage,
    };
    vec![v(0), v(1), v(2), v(0), v(2), v(3)]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn centre(img: &[u8]) -> [u8; 3] {
    let i = (((H / 2) * W + W / 2) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

fn check_pixels(c: &mut Checker, gpu: &mut Gpu, args: &BreakshotArgs) -> Result<(), String> {
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }
    let mut off = Offscreen::new(gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    // A mid-grey block texture: the multiply has to have something to halve.
    let block: Vec<u8> = (0..(TEX * TEX))
        .flat_map(|_| [BLOCK_GREY, BLOCK_GREY, BLOCK_GREY, 255])
        .collect();
    let layers = [block];
    let mut wr = WorldRenderer::new(gpu, off.format, TEX, &layers)?;
    let result = run_pixels(c, gpu, &mut off, &mut wr, &draw, args);
    wr.destroy(gpu);
    off.destroy(gpu);
    result
}

fn run_pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    wr: &mut WorldRenderer,
    draw: &OverlayDraw,
    args: &BreakshotArgs,
) -> Result<(), String> {
    wr.init_crumbling(gpu, &grey_stages(), TEX)?;
    c.record(
        "c1.the_crumbling_pass_exists",
        wr.crumbling_ready(),
        "built against the UNORM counterpart of the attachment format, because the \
         multiply blend has to run on gamma-encoded numbers",
    );

    let (verts, indices) = floor_quad();
    let vb: &[u8] = bytemuck::cast_slice(&verts);
    wr.upload_column(gpu, 0, 0, vb, &indices, &[], &[], 64.0, 66.0)?;
    gpu.wait_idle();

    // Straight down at the block's top face, filling the middle of the frame.
    let eye = Vec3::new(0.5, 66.0, 0.5);
    wr.set_camera([eye.x, eye.y, eye.z]);
    let view = Mat4::look_to_rh(eye, Vec3::new(0.0, -1.0, 0.0), Vec3::Z);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let vp = (proj * view).to_cols_array_2d();
    wr.set_fog(1.0e6, 1.0e6 + 1.0);
    wr.set_sky_tint([0.0; 3], [0.0; 3]);

    let shot = |gpu: &mut Gpu,
                off: &mut Offscreen,
                wr: &mut WorldRenderer,
                v: &[CrumblingVertex],
                name: &str|
     -> Result<Vec<u8>, String> {
        wr.set_crumbling(gpu, v)?;
        off.render(gpu, Some((&mut *wr, vp)), draw, CLEAR)?;
        let img = off.read_rgba(gpu)?;
        if let Some(d) = &args.out_dir {
            let _ = off.save_png(gpu, &d.join(format!("{name}.png")));
        }
        Ok(img)
    };

    let plain = shot(gpu, off, wr, &[], "plain")?;
    let bare = centre(&plain);
    c.record(
        "c2.the_uncracked_block_renders",
        bare.iter().any(|&v| v > 20),
        format!("the block's top face reads {bare:?} with no decal"),
    );

    // **Byte-identical**, not merely similar: an empty decal list must not
    // touch the frame at all.
    let again = shot(gpu, off, wr, &[], "plain2")?;
    c.record(
        "c3.no_record_leaves_the_frame_byte_identical",
        again == plain,
        "two frames with no crumbling geometry are the same bytes",
    );

    let s1 = shot(gpu, off, wr, &decal_verts(1), "stage1")?;
    let s9 = shot(gpu, off, wr, &decal_verts(9), "stage9")?;
    let (c1v, c9) = (centre(&s1), centre(&s9));
    c.record(
        "c4.a_dark_crack_darkens_the_block",
        (0..3).all(|i| c1v[i] < bare[i]) && (0..3).all(|i| c9[i] < bare[i]),
        format!(
            "bare {bare:?} → stage 1 {c1v:?} → stage 9 {c9:?} — a decal texel below \
             mid-grey halves what is under it"
        ),
    );

    // MUTATION: a pass that ignored the per-vertex stage would render both
    // rows identically.
    c.record(
        "c5.a_later_stage_darkens_more",
        (0..3).all(|i| c9[i] < c1v[i]),
        format!(
            "stage 1 {c1v:?} vs stage 9 {c9:?} — the stage really is selecting an \
             array layer, not decorating the vertex"
        ),
    );

    // The gamma question. `2·src·dst` on the stored bytes against the same
    // multiply on linearised values, encoded back — two different numbers, and
    // a witness pins which one the GPU produced. M50's finding, second place.
    let src = dark_stage(1) as f32 / 255.0;
    let c0 = c1v;
    let want_gamma: Vec<u8> = bare
        .iter()
        .map(|&d| {
            let v = 2.0 * src * (d as f32 / 255.0);
            (v.min(1.0) * 255.0).round() as u8
        })
        .collect();
    let want_linear: Vec<u8> = bare
        .iter()
        .map(|&d| {
            let v = 2.0 * srgb_to_linear(src) * srgb_to_linear(d as f32 / 255.0);
            (linear_to_srgb(v.min(1.0)) * 255.0).round() as u8
        })
        .collect();
    let near_gamma = (0..3).all(|i| (c0[i] as i32 - want_gamma[i] as i32).abs() <= 3);
    c.record(
        "c6.the_multiply_runs_in_gamma_space",
        near_gamma,
        format!(
            "GPU {c0:?}; a gamma-space 2·src·dst predicts {want_gamma:?}; a \
             linear-space one predicts {want_linear:?} — vanilla has no sRGB \
             framebuffer, so the encoded bytes are what it multiplies"
        ),
    );
    c.record(
        "c7.the_two_spaces_are_distinguishable_here",
        (0..3).any(|i| (want_gamma[i] as i32 - want_linear[i] as i32).abs() > 3),
        format!(
            "gamma {want_gamma:?} vs linear {want_linear:?} — if these agreed, c6 \
             would be asserting nothing"
        ),
    );

    // **The blend brightens above mid-grey**, and that is not a defect: the
    // output is `2·src·dst`, so `src > 0.5` scales the destination up. It is
    // the reason `destroy_stage` is authored as dark lines on transparency
    // rather than as a light overlay — a pale crack would make the block glow.
    //
    // The first version of c4 claimed the opposite ("the decal cannot brighten
    // a texel"), used light greys, and measured 255. The measurement was right.
    //
    // MUTATION: an ordinary alpha blend cannot brighten at all, and would put
    // this row somewhere between the block and the decal grey.
    let light = shot(gpu, off, wr, &decal_verts(0), "stage_light")?;
    let cl = centre(&light);
    c.record(
        "c8.a_decal_texel_above_mid_grey_brightens_rather_than_darkens",
        (0..3).all(|i| cl[i] > bare[i]),
        format!(
            "a {LIGHT_STAGE}-grey decal over a {BLOCK_GREY}-grey block gives {cl:?} — \
             2·src·dst with src > 0.5 scales up, which is why the real artwork is dark"
        ),
    );

    wr.remove_column(gpu, 0, 0);
    Ok(())
}
