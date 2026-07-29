//! `rewo mobshot` — serverless mob-model verification (headless, no MC
//! server needed).
//!
//! Two modes:
//! - **Contact sheet** (default): render every available mob with its real
//!   texture into one PNG — the eyeball artifact.
//! - **`--check`**: the facelabel gate (REWO_MOB_REDO_HANDOFF §6). Every
//!   mob texture is replaced by per-face solid colors
//!   (`REWO_MOB_DEBUG_TEX`), each mob renders from front/left/top, and the
//!   dominant rendered color must match the dominant face *predicted from
//!   the model geometry itself* (projected camera-facing area by face
//!   label). A scrambled UV unwrap makes the rendered color diverge from
//!   the geometric prediction — the exact bug class the old hand-rolled
//!   unwrap shipped. Runs twice per view (with and without the mob) and
//!   only classifies pixels that differ, so sky/fog/overlay can't pollute
//!   the counts.
//!
//! In-rect rotations/flips are invisible to solid colors — those are pinned
//! by the exact per-vertex UV unit tests in `rewo_gpu::mobs`.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::{assets, DataPaths};
use rewo_gpu::entities::EntityDraw;
use rewo_gpu::mobs::{EntityModelKind, Facing};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

const BG: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
const FACES: [Facing; 6] = [
    Facing::Down,
    Facing::Up,
    Facing::West,
    Facing::North,
    Facing::East,
    Facing::South,
];

#[derive(ClapArgs)]
pub struct MobshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Contact-sheet output (default mode) or, with --check, unused.
    #[arg(long, default_value = "rewo-mobs.png")]
    out: PathBuf,
    /// Facelabel verification: assert texture-face correspondence for every
    /// mob from 3 angles; nonzero exit on any mismatch.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// With --check: also dump each per-view debug render here.
    #[arg(long)]
    out_dir: Option<PathBuf>,
    /// Sheet mode: render only these mobs (comma-separated kind names) in a
    /// closer 3/4 view — the detail-inspection artifact.
    #[arg(long)]
    only: Option<String>,
    /// Sheet mode: ambient-animation time in seconds (wing flutter, rod
    /// orbits, tentacle sway). The --check gate always uses 0.
    #[arg(long, default_value_t = 0.0)]
    time: f32,
    /// Sheet mode: walk pose as "swing,amount" (drives leg/arm gaits,
    /// spider leg waves, tail wags). The --check gate always uses 0.
    #[arg(long)]
    walk: Option<String>,
    /// Sheet mode: seconds of animation to simulate at 20 Hz before rendering.
    /// A resource pack's variables are integrators that converge over time
    /// (CEM `var.run`, `var.air`, …), so a single frame catches them barely
    /// off zero; settling first renders the steady-state pose you actually see
    /// in game. No effect without a pack.
    #[arg(long, default_value_t = 0.0)]
    settle: f32,
    /// Sheet mode: world-light multiplier `0..1` applied to the mob, as the
    /// live client samples from the world. 1.0 (default) is fullbright; lower
    /// shows how a mob reads in shade without needing a server.
    #[arg(long, default_value_t = 1.0)]
    light: f32,
    /// Sheet mode: pose/state gesture as "name[,age_s]" (e.g.
    /// "warden_roar,1.5") — plays the one-shot rig at that clock on every
    /// mob it applies to. Names match `Gesture::from_name`.
    #[arg(long)]
    gesture: Option<String>,
    /// Sheet mode: render the armadillo hiding in its shell (the
    /// visibility swap the gestures drive in live play).
    #[arg(long, default_value_t = false)]
    shell: bool,
    /// Sheet mode: pose the Allay dancing at this `spinningProgress` (0..1) —
    /// `is_spinning = progress > 0` (so `0` = the swaying pose, a positive value
    /// = the spin at that progress). Combine with `--time` for the beat phase.
    /// The `--check` gate never dances (that is `danceshot`'s job).
    #[arg(long)]
    dance: Option<f32>,
    /// Sheet mode: fetch a real player skin (a Minecraft username or a raw
    /// texture URL) and render the Player model with it — the headless M7c
    /// verification (`--only player --skin <name>`). Slim/wide is taken
    /// from the profile.
    #[arg(long)]
    skin: Option<String>,
    /// Load an OptiFine CEM resource-pack zip and render mobs with their
    /// pack models (M9). Combine with `--only <mob>` to inspect one.
    #[arg(long)]
    pack: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
    /// Emissive gate (M57): assert every mob's vanilla emissive layers ignore
    /// world light, and that mobs without such layers go fully black in the
    /// dark. Nonzero exit on any mismatch.
    #[arg(long, default_value_t = false)]
    emissive_check: bool,
    /// ETF gate (M57): build a fixture resource pack and assert its
    /// random-entity variants land on the right mob and the right texture slot.
    #[arg(long, default_value_t = false)]
    etf_check: bool,
    /// Dye gate (M57): assert every dye renders vanilla's wool colour and only
    /// tints the wool.
    #[arg(long, default_value_t = false)]
    tint_check: bool,
    /// Variant gate (M64): assert vanilla's metadata-driven texture variants
    /// bake, decode, route and render — cat, wolf, frog, axolotl, horse, llama.
    #[arg(long, default_value_t = false)]
    variant_check: bool,
    /// Sheet mode: `Warden.getTendrilAnimation` 0..1 — the countdown
    /// entity_event 61 starts. Sways the tendrils and lights their emissive
    /// layer.
    #[arg(long, default_value_t = 0.0)]
    tendril: f32,
    /// Sheet mode: render mobs with this pack texture variant (ETF / M57). 0 is
    /// the vanilla texture; needs `--pack`.
    #[arg(long, default_value_t = 0)]
    variant: u16,
    /// Sheet mode: dye colour 0..15 for a mob's tinted texture — the sheep's
    /// wool (vanilla `SheepWoolLayer`).
    #[arg(long)]
    dye: Option<u8>,
    /// Sheet mode: `Creaking.isActive()` — lights the creaking's eyes.
    #[arg(long, default_value_t = false)]
    eyes_glow: bool,
}

pub fn run(args: MobshotArgs) -> Result<(), String> {
    if args.check {
        // Must be set before the entity pass bakes its atlas.
        std::env::set_var("REWO_MOB_DEBUG_TEX", "1");
    }
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut gpu = Gpu::new(None, cfg!(debug_assertions) && !args.no_validation)?;

    let result = if args.check {
        run_check(&mut gpu, &baked, &args)
    } else if args.emissive_check {
        run_emissive_check(&mut gpu, &baked, &args)
    } else if args.etf_check {
        run_etf_check(&mut gpu, &baked, &args)
    } else if args.tint_check {
        run_tint_check(&mut gpu, &baked, &args)
    } else if args.variant_check {
        run_variant_check(&mut gpu, &baked, &args)
    } else {
        run_sheet(&mut gpu, &baked, &args)
    };
    result
}

/// Hidden overlay: the offscreen render always draws the frame-time chart,
/// so park it far outside the viewport.
fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

/// Parse a resource-pack's CEM `.jem` models into per-kind model overrides
/// (M9). Files whose entity name doesn't map to a known model kind, or that
/// fail to parse, are skipped with a notice.
pub(crate) fn load_cem_overrides(
    path: &std::path::Path,
) -> Result<std::collections::HashMap<EntityModelKind, rewo_gpu::mobs::Model>, String> {
    let pack = rewo_data::cem::load_pack(path)?;
    let mut out = std::collections::HashMap::new();
    for file in &pack.files {
        let kind = rewo_gpu::mobs::kind_for_entity_name(&format!("minecraft:{}", file.entity));
        if kind == EntityModelKind::Capsule {
            continue; // no matching model kind (variant/collar/… files)
        }
        match rewo_gpu::cem::model_from_jem_for(&file.entity, &file.jem, &pack.jpms) {
            Ok(model) => {
                out.entry(kind).or_insert(model);
            }
            Err(e) => log::warn!("cem: {} skipped: {e}", file.entity),
        }
    }
    println!("[mobshot] pack: {} CEM model(s) mapped to kinds", out.len());
    Ok(out)
}

fn neutral_draw(kind: EntityModelKind) -> EntityDraw<'static> {
    EntityDraw {
        pos: [0.0; 3],
        width: 1.0,
        height: 2.0,
        color: [1.0, 0.0, 1.0],
        name: None,
        // M59: no health bar in a still — the gate renders its own.
        health: None,
        kind,
        yaw: 0.0,
        death_time: 0.0,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None, None],
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        // Stills are fullbright because there is no world lightmap to sample.
        light: [1.0, 1.0, 1.0],
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    }
}

// ---------------------------------------------------------------------------
// --check: the facelabel gate
// ---------------------------------------------------------------------------

fn run_check(gpu: &mut Gpu, baked: &assets::BakedAssets, args: &MobshotArgs) -> Result<(), String> {
    let (w, h) = (512u32, 512u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(gpu, crate::live_cmd::font_data(baked), crate::live_cmd::entity_textures(baked))?;
    let mut kinds = wr.entity_pass().expect("entity pass").available_kinds();
    // Textures that reuse the same texels across face labels (breeze wind's
    // concentric shells) can't be color-checked — skip those mobs loudly.
    let ambiguous = wr.entity_pass().expect("entity pass").debug_ambiguous_kinds().to_vec();
    for k in &ambiguous {
        println!(
            "[mobshot] SKIP {:?}: texture reuses rects across face labels — facelabel check N/A",
            k
        );
    }
    kinds.retain(|k| !ambiguous.contains(k));
    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
    }

    // (label, view direction the camera looks along, up vector).
    let views: [(&str, Vec3, Vec3); 3] = [
        ("front", Vec3::new(0.0, 0.0, -1.0), Vec3::Y),
        ("left", Vec3::new(-1.0, 0.0, 0.0), Vec3::Y),
        ("top", Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 0.0, -1.0)),
    ];
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for kind in kinds {
        let quads = wr
            .entity_pass()
            .expect("entity pass")
            .neutral_quads(kind)
            .expect("kind was listed available");
        // World-space bbox of the neutral pose (entity at origin, yaw 0).
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for (pos, _, _) in &quads {
            for p in pos {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
        let center = Vec3::new(
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        );
        let radius = (0..3).map(|i| hi[i] - lo[i]).fold(0f32, f32::max) * 0.5;
        let dist = radius * 3.0 + 1.0;

        for (view_name, dir, up) in views {
            let eye = center - dir * dist;
            let predicted = predict_counts(&quads, eye, dir, up);
            let (pi, &pmax) = predicted
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| **c)
                .expect("6 faces");
            if pmax == 0 {
                failures.push(format!("{kind:?}/{view_name}: no camera-facing quads"));
                continue;
            }
            let expected = FACES[pi];
            let view = Mat4::look_to_rh(eye, dir, up);
            let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
                55f32.to_radians(),
                1.0,
                0.05,
            ));
            let vp = (proj * view).to_cols_array_2d();
            wr.set_camera(eye.to_array());
            let right = dir.cross(Vec3::Y).normalize_or_zero().to_array();
            let upv = up.to_array();

            // Background-only pass, then the mob; classify only changed
            // pixels so sky/fog/chart can't leak into the counts.
            wr.set_entities(&[], right, upv, 0.0);
            off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
            let bg = off.read_rgba(gpu)?;
            wr.set_entities(&[neutral_draw(kind)], right, upv, 0.0);
            off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
            let img = off.read_rgba(gpu)?;
            if let Some(dir) = &args.out_dir {
                save_png(&img, w, h, &dir.join(format!("{}-{view_name}.png", kind_name(kind))))?;
            }

            let mut counts = [0usize; 6];
            let mut changed = 0usize;
            for i in (0..img.len()).step_by(8) {
                if img[i..i + 3] != bg[i..i + 3] {
                    changed += 1;
                    if let Some(f) = classify(&img[i..i + 4]) {
                        counts[face_index(f)] += 1;
                    }
                }
            }
            let (di, &dc) = counts
                .iter()
                .enumerate()
                .max_by_key(|(_, c)| **c)
                .expect("6 faces");
            checked += 1;
            // Strict when the prediction is unambiguous; a near-tie (the
            // rendered dominant predicted within 80% of the predicted max,
            // e.g. the enderman's half-mirrored left profile) passes too —
            // a scrambled unwrap still scores ~0 and fails hard.
            let near_tie = predicted[di] * 5 >= pmax * 4;
            if changed < 400 {
                failures.push(format!(
                    "{kind:?}/{view_name}: mob barely visible ({changed} px changed)"
                ));
            } else if dc == 0 || (FACES[di] != expected && !near_tie) {
                failures.push(format!(
                    "{kind:?}/{view_name}: expected {expected:?} (predicted {predicted:?}), rendered dominant {:?} (counts {counts:?})",
                    FACES[di]
                ));
            }
        }
    }

    wr.destroy(gpu);
    off.destroy(gpu);
    if failures.is_empty() {
        println!("[mobshot] CHECK OK — {checked} mob-views match their geometric face labels");
        Ok(())
    } else {
        for f in &failures {
            println!("[mobshot] FAIL {f}");
        }
        Err(format!("mobshot check: {} of {checked} views failed", failures.len()))
    }
}

/// Per-face-label visibility counts by ray-casting the **same perspective
/// camera** the render uses (same eye, direction, fov), keeping each ray's
/// nearest hit — a tiny reference renderer of labels. Occlusion and
/// projection match the real render exactly (a chicken from above shows its
/// rotated body's South rect, a villager its hat rim, a zombie's high hat
/// beats its outstretched arms). Purely geometric — independent of the UV
/// path under test.
fn predict_counts(
    quads: &[([[f32; 3]; 4], Facing, [f32; 3])],
    eye: Vec3,
    dir: Vec3,
    up: Vec3,
) -> [usize; 6] {
    let right = dir.cross(up).normalize();
    let real_up = right.cross(dir);
    let half = (55f32.to_radians() / 2.0).tan();
    const N: usize = 128;
    let mut counts = [0usize; 6];
    for iy in 0..N {
        for ix in 0..N {
            let sx = (2.0 * (ix as f32 + 0.5) / N as f32 - 1.0) * half;
            let sy = (1.0 - 2.0 * (iy as f32 + 0.5) / N as f32) * half;
            let rd = (dir + right * sx + real_up * sy).normalize();
            let mut best = (f32::MAX, None);
            for (pos, facing, _) in quads {
                let p: [Vec3; 4] = [
                    Vec3::from_array(pos[0]),
                    Vec3::from_array(pos[1]),
                    Vec3::from_array(pos[2]),
                    Vec3::from_array(pos[3]),
                ];
                for tri in [[0, 1, 2], [0, 2, 3]] {
                    if let Some(t) = ray_tri(eye, rd, p[tri[0]], p[tri[1]], p[tri[2]]) {
                        // First-emitted wins near-ties — matches the
                        // renderer, where coplanar faces of 0-thick plates
                        // rasterize at equal depth and the strict depth
                        // test keeps the first-drawn quad.
                        if t < best.0 - 2e-5 {
                            best = (t, Some(*facing));
                        }
                    }
                }
            }
            if let Some(f) = best.1 {
                counts[face_index(f)] += 1;
            }
        }
    }
    counts
}

/// Möller–Trumbore, both-sided (mirrored cubes flip winding).
fn ray_tri(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let uu = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&uu) {
        return None;
    }
    let q = s.cross(e1);
    let vv = dir.dot(q) * inv;
    if vv < 0.0 || uu + vv > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

/// Map a rendered debug texel back to its face label by chroma pattern —
/// robust under the per-face shade multiply and the sRGB round-trip.
fn classify(px: &[u8]) -> Option<Facing> {
    let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
    let max = r.max(g).max(b);
    if max < 40 {
        return None;
    }
    let bit = |c: i32| c * 2 > max;
    match (bit(r), bit(g), bit(b)) {
        (true, false, false) => Some(Facing::North),
        (true, true, false) => Some(Facing::South),
        (false, true, false) => Some(Facing::West),
        (false, false, true) => Some(Facing::East),
        (true, false, true) => Some(Facing::Down),
        (false, true, true) => Some(Facing::Up),
        _ => None,
    }
}

fn face_index(f: Facing) -> usize {
    FACES.iter().position(|x| *x == f).unwrap()
}

fn kind_name(k: EntityModelKind) -> &'static str {
    k.name()
}

// ---------------------------------------------------------------------------
// Contact sheet
// ---------------------------------------------------------------------------

fn run_sheet(gpu: &mut Gpu, baked: &assets::BakedAssets, args: &MobshotArgs) -> Result<(), String> {
    let (w, h) = (2560u32, 1440u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    // --pack: the pack's CEM .jem models override the matching kinds, and its
    // ETF alternates join the atlas so `--variant` can select one.
    let cem = match &args.pack {
        Some(path) => load_cem_overrides(path)?,
        None => std::collections::HashMap::new(),
    };
    let etf = match &args.pack {
        Some(path) => rewo_data::etf::load_pack(path)?,
        None => rewo_data::etf::EtfPack::default(),
    };
    let tex = crate::live_cmd::entity_textures_with(baked, &etf);
    if cem.is_empty() {
        wr.init_entities(gpu, crate::live_cmd::font_data(baked), tex)?;
    } else {
        wr.init_entities_with_cem(gpu, crate::live_cmd::font_data(baked), tex, cem)?;
    }
    // --skin: fetch a real player skin, upload it, remember its UV offset +
    // model so the Player draw wears it (the M7c verification path).
    let player_skin: Option<([f32; 2], bool)> = match &args.skin {
        Some(name) => {
            let info = crate::skin_fetch::resolve(name)?;
            let url = info.url.as_deref().ok_or("profile carries no skin")?;
            let rgba = crate::skin_fetch::fetch_rgba64(url)?;
            let uv = wr.upload_player_skin(gpu, &rgba).ok_or("skin upload failed")?;
            println!("[mobshot] skin: {name} → {} model, uploaded", if info.slim { "slim" } else { "wide" });
            Some((uv, info.slim))
        }
        None => None,
    };
    let pass = wr.entity_pass().expect("entity pass");
    let mut kinds = pass.available_kinds();
    if let Some(only) = &args.only {
        let wanted: Vec<&str> = only.split(',').map(str::trim).collect();
        kinds.retain(|k| wanted.contains(&k.name()));
        if kinds.is_empty() {
            return Err(format!("--only matched no mobs: {only}"));
        }
    }
    // With --skin, render the player row as the model the profile specifies.
    if let Some((_, slim)) = player_skin {
        for k in kinds.iter_mut() {
            if matches!(k, EntityModelKind::Player | EntityModelKind::PlayerSlim) {
                *k = if slim { EntityModelKind::PlayerSlim } else { EntityModelKind::Player };
            }
        }
        kinds.dedup();
    }
    println!(
        "[mobshot] {} models: {}",
        kinds.len(),
        kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(", ")
    );
    // Short mobs in the front rows, giants (ghast, golem, camel) in the
    // back — nothing hides behind something taller.
    let height_of = |k: &rewo_gpu::mobs::EntityModelKind| -> f32 {
        pass.neutral_quads(*k)
            .map(|qs| {
                qs.iter()
                    .flat_map(|(pos, _, _)| pos.iter().map(|p| p[1]))
                    .fold(0.0f32, f32::max)
            })
            .unwrap_or(2.0)
    };
    kinds.sort_by(|a, b| height_of(a).total_cmp(&height_of(b)));

    // Grid facing the camera (mobs face +Z at yaw 0; camera sits south).
    // Rows step away from the camera; odd rows stagger half a column so
    // nobody hides directly behind the mob in front.
    let cols = 10usize;
    let (sx, sz) = (3.8f32, 6.5f32);
    let rows = kinds.len().div_ceil(cols);
    let (walk_swing, walk_amt) = args
        .walk
        .as_deref()
        .and_then(|s| {
            let mut it = s.split(',');
            Some((it.next()?.trim().parse().ok()?, it.next()?.trim().parse().ok()?))
        })
        .unwrap_or((0.0, 0.0));
    let gesture = match args.gesture.as_deref() {
        Some(s) => {
            let mut it = s.split(',');
            let name = it.next().unwrap_or("").trim();
            let g = rewo_gpu::mobs::Gesture::from_name(name)
                .ok_or_else(|| format!("unknown --gesture {name:?}"))?;
            let age = it.next().and_then(|a| a.trim().parse().ok()).unwrap_or(0.0);
            Some((g, age))
        }
        None => None,
    };
    let draws: Vec<EntityDraw<'_>> = kinds
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let mut d = neutral_draw(*k);
            let light = args.light.clamp(0.0, 1.0);
            d.light = [light, light, light];
            d.limb_swing = walk_swing;
            d.limb_amount = walk_amt;
            d.gesture = gesture;
            d.shell = args.shell;
            d.variant = args.variant;
            d.dye = args.dye;
            d.emissive = rewo_gpu::entities::EmissiveState {
                tendril: args.tendril.clamp(0.0, 1.0),
                eyes_glow: args.eyes_glow,
            };
            if *k == EntityModelKind::Allay {
                d.allay_dance = args.dance.map(|spin| rewo_gpu::mobs::AllayDance {
                    is_spinning: spin > 0.0,
                    spinning_progress: spin,
                });
            }
            if matches!(k, EntityModelKind::Player | EntityModelKind::PlayerSlim) {
                if let Some((uv, _)) = player_skin {
                    d.skin_uv = Some(uv);
                }
            }
            let (row, col) = (i / cols, i % cols);
            let row_n = ((kinds.len() - row * cols).min(cols)) as f32;
            let stagger = if row % 2 == 1 { sx * 0.5 } else { 0.0 };
            d.pos = [
                (col as f32 - (row_n - 1.0) / 2.0) * sx + stagger,
                0.0,
                -(row as f32) * sz,
            ];
            d
        })
        .collect();

    let max_h = kinds.iter().map(|k| height_of(k)).fold(2.0f32, f32::max);
    let center = Vec3::new(0.0, (max_h * 0.45).max(1.0), -((rows as f32 - 1.0) * sz) / 2.0);
    let eye = if args.only.is_some() {
        // Closeup: 3/4 view fitted to the row width + tallest mob.
        let span = (kinds.len().min(cols) as f32) * sx;
        let dist = (span * 0.55).max(max_h * 1.9) + 3.0;
        center + Vec3::new(dist * 0.4, dist * 0.35, dist)
    } else {
        center + Vec3::new(0.0, 11.0, (rows as f32) * sz * 0.6 + 12.0)
    };
    let dir = (center - eye).normalize();
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        55f32.to_radians(),
        w as f32 / h as f32,
        0.05,
    ));
    let vp = (proj * view).to_cols_array_2d();
    let right = dir.cross(Vec3::Y).normalize_or_zero().to_array();
    let up = right_up(dir);
    // Settle the pack's animation integrators before the shot: CEM variables
    // accumulate frame-to-frame, so one evaluation leaves them barely off zero.
    // Stepping a 20 Hz clock up to `--time` converges them the way live play
    // does. Cheap (vertex-soup rebuild only) and a no-op at the default 0.
    let steps = (args.settle * 20.0).round().max(0.0) as usize;
    for i in 0..steps {
        wr.set_entities(&draws, right, up, args.time + i as f32 / 20.0);
    }
    wr.set_entities(&draws, right, up, args.time + args.settle);

    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    for _ in 0..3 {
        off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
    }
    off.save_png(gpu, &args.out)?;
    println!("[mobshot] sheet: {} mobs, wrote {}", kinds.len(), args.out.display());
    wr.destroy(gpu);
    off.destroy(gpu);
    Ok(())
}

fn right_up(dir: Vec3) -> [f32; 3] {
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    right.cross(dir).normalize_or_zero().to_array()
}

fn save_png(rgba: &[u8], w: u32, h: u32, path: &PathBuf) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wtr| wtr.write_image_data(rgba))
        .map_err(|e| format!("png {path:?}: {e}"))?;
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

// ===========================================================================
// M57 gates: --emissive-check, --etf-check, --tint-check
// ===========================================================================

/// Named-observation accumulator, the M17+ convention (`itemshot_cmd.rs`).
/// Each property increments `witnessed` only on a real pass; a failure is
/// recorded without incrementing, so a run reports every bad property and the
/// count-vs-expected guard catches a *skipped* one.
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
        println!(
            "[mobshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }

    fn finish(self, gate: &str, expected: usize) -> Result<(), String> {
        println!("[mobshot] {gate} witnesses observed: {} / {expected}", self.witnessed);
        if !self.failures.is_empty() {
            return Err(format!(
                "{} propert{} failed: {}",
                self.failures.len(),
                if self.failures.len() == 1 { "y" } else { "ies" },
                self.failures.join(", ")
            ));
        }
        if self.witnessed != expected {
            return Err(format!(
                "witness count {} != expected {expected} — a named property was \
                 skipped (fail-closed)",
                self.witnessed
            ));
        }
        println!("[mobshot] {gate} PASS — {} witnesses", self.witnessed);
        Ok(())
    }
}

/// Frame one mob the way the facelabel gate does, returning `(view_proj, right,
/// up, eye)`. Shared by all three M57 gates so a camera difference can never be
/// the reason one of them disagrees with another.
fn frame_kind(wr: &WorldRenderer, kind: EntityModelKind) -> Option<([[f32; 4]; 4], [f32; 3], [f32; 3], Vec3)> {
    let quads = wr.entity_pass()?.neutral_quads(kind)?;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (pos, _, _) in &quads {
        for p in pos {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
    }
    let center = Vec3::new((lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5);
    let radius = (0..3).map(|i| hi[i] - lo[i]).fold(0f32, f32::max) * 0.5;
    let dir = Vec3::new(0.0, 0.0, -1.0);
    let eye = center - dir * (radius * 3.0 + 1.0);
    let vp = (Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        55f32.to_radians(),
        1.0,
        0.05,
    )) * Mat4::look_to_rh(eye, dir, Vec3::Y))
    .to_cols_array_2d();
    let right = dir.cross(Vec3::Y).normalize_or_zero().to_array();
    Some((vp, right, [0.0, 1.0, 0.0], eye))
}

// ---------------------------------------------------------------------------
// --emissive-check: the fullbright gate
// ---------------------------------------------------------------------------

/// Named properties this gate observes. Fail-closed: a skipped one is an error.
const EMISSIVE_WITNESSES: usize = 5;

/// A pixel counts as *glowing* when any channel clears this. The base model
/// renders as exactly `texel · 0` at world light 0 — pure black — so anything
/// above the sRGB round-trip noise floor came from an emissive layer.
const GLOW_LEVEL: u8 = 12;
/// A mob must cover at least this many pixels to be worth grading.
const MIN_SILHOUETTE: usize = 400;
/// The age (ticks) the emissive gate renders at, chosen so that
/// `cos(age · 2.25) == 0` — the exact zero of `WardenModel.animateTendrils`.
///
/// This matters. `EmissiveState::tendril` feeds **both** the tendril sway and
/// the tendril layer's alpha, so at a generic age raising it moves the tendrils
/// *and* lights them. An early version of this gate compared glow pixel counts
/// across the two states and was fooled by the movement alone — rotating
/// tendrils uncover a few pixels of the (glowing) head behind them, so a build
/// with the alpha hard-wired to 0 still "grew". Freezing the sway isolates the
/// alpha path, which is what the assertion claims to test.
const CHECK_AGE_TICKS: f32 = std::f32::consts::FRAC_PI_2 / 2.25;

/// The emissive gate: **an emissive layer ignores world light, and nothing else
/// does.**
///
/// For every mob, render it once lit (the silhouette) and once at world light
/// 0. At light 0 the base model is multiplied to black, so any pixel that is
/// still bright can only have come from an emissive layer. That gives a
/// two-sided assertion over the whole registry:
///
/// - a mob whose `mobs::emissive_layers` are active at this state **must** show
///   bright pixels in the dark, and they must lie inside its silhouette (a
///   layer drifting off its model would show up here);
/// - every other mob — 80-odd of them — **must** be perfectly black. That
///   control half is what makes this a gate rather than a rubber stamp: it
///   fails if the emissive pass leaks onto the wrong mob, or if the base pass
///   ever stops respecting world light.
///
/// The state-driven layers (the warden's tendrils, the creaking's eyes) get a
/// third render with their state raised, and the glow must strictly grow —
/// proving the alpha functions actually reach the draw rather than sitting at a
/// constant.
///
/// Four mutations were run against it: emissive respecting light (8 mobs fail),
/// the tendril alpha pinned to 0 (1), a layer leaked onto another mob (1), and
/// the base pass ignoring light (84).
fn run_emissive_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    _args: &MobshotArgs,
) -> Result<(), String> {
    use rewo_gpu::entities::EmissiveState;
    use rewo_gpu::mobs::EmissiveAlpha;

    let (w, h) = (512u32, 512u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(
        gpu,
        crate::live_cmd::font_data(baked),
        crate::live_cmd::entity_textures(baked),
    )?;
    let kinds = wr.entity_pass().expect("entity pass").available_kinds();
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    let mut c = Checker::new();

    // Per-property failure lists, so one bad mob names itself instead of
    // collapsing 89 observations into one opaque boolean.
    let (mut dark_fail, mut black_fail, mut stray_fail, mut state_fail) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut checked, mut glowing, mut expected_glowing, mut state_driven_n) = (0usize, 0usize, 0usize, 0usize);
    let age = CHECK_AGE_TICKS;

    for kind in kinds {
        let layers = rewo_gpu::mobs::emissive_layers(kind);
        // What the decompiled alpha functions say this mob does at rest
        // (vanilla synched defaults) — computed from the layer table itself,
        // independently of the renderer.
        let rest_alpha = |a: EmissiveAlpha| -> f32 {
            match a {
                EmissiveAlpha::Always => 1.0,
                EmissiveAlpha::PulsatingSpots { phase } => {
                    ((age * 0.045 + phase).cos() * 0.25).max(0.0)
                }
                // Both are 0 at their vanilla defaults (tendril countdown spent,
                // IS_ACTIVE false).
                EmissiveAlpha::Tendril | EmissiveAlpha::EyesGlowing => 0.0,
                // `heartAnimation` counts 10 down to 0 over the ticks after each
                // beat.
                EmissiveAlpha::Heart => ((10.0 - age) / 10.0).max(0.0),
            }
        };
        let expect_rest = layers.iter().any(|l| {
            let a = rest_alpha(l.alpha);
            a > 1.0e-5 && !(l.cutout && a < 0.1)
        });
        let state_driven = layers
            .iter()
            .any(|l| matches!(l.alpha, EmissiveAlpha::Tendril | EmissiveAlpha::EyesGlowing));

        let Some((vp, right, up, eye)) = frame_kind(&wr, kind) else {
            continue;
        };
        wr.set_camera(eye.to_array());

        let mut shoot = |wr: &mut WorldRenderer,
                         gpu: &mut Gpu,
                         off: &mut Offscreen,
                         entity: Option<(f32, EmissiveState)>|
         -> Result<Vec<u8>, String> {
            match entity {
                Some((light, emissive)) => {
                    let d = EntityDraw {
                        light: [light; 3],
                        emissive,
                        ..neutral_draw(kind)
                    };
                    wr.set_entities(std::slice::from_ref(&d), right, up, age / 20.0);
                }
                None => wr.set_entities(&[], right, up, age / 20.0),
            }
            off.render(gpu, Some((wr, vp)), &draw, BG)?;
            off.read_rgba(gpu)
        };
        let bg = shoot(&mut wr, gpu, &mut off, None)?;
        let lit = shoot(&mut wr, gpu, &mut off, Some((1.0, EmissiveState::default())))?;
        let dark = shoot(&mut wr, gpu, &mut off, Some((0.0, EmissiveState::default())))?;

        let silhouette: Vec<bool> = (0..bg.len() / 4)
            .map(|i| lit[i * 4..i * 4 + 3] != bg[i * 4..i * 4 + 3])
            .collect();
        let sil_px = silhouette.iter().filter(|v| **v).count();
        let glow = |img: &[u8]| -> (usize, usize) {
            let (mut inside, mut outside) = (0, 0);
            for i in 0..img.len() / 4 {
                let px = &img[i * 4..i * 4 + 3];
                if px.iter().all(|c| *c < GLOW_LEVEL) {
                    continue;
                }
                // Outside the silhouette everything is sky, which is bright —
                // only count pixels the mob covers.
                if silhouette[i] {
                    inside += 1;
                } else if img[i * 4..i * 4 + 3] != bg[i * 4..i * 4 + 3] {
                    outside += 1;
                }
            }
            (inside, outside)
        };
        let (dark_glow, stray) = glow(&dark);
        checked += 1;
        if dark_glow > 0 {
            glowing += 1;
        }
        if expect_rest {
            expected_glowing += 1;
        }
        if sil_px < MIN_SILHOUETTE {
            black_fail.push(format!("{kind:?} barely visible ({sil_px} px)"));
            continue;
        }
        if stray > 0 {
            stray_fail.push(format!("{kind:?}: {stray} px"));
        }
        match (expect_rest, dark_glow) {
            (true, 0) => dark_fail.push(format!("{kind:?}")),
            (false, n) if n > 0 => black_fail.push(format!("{kind:?}: {n} px")),
            _ => {}
        }
        if state_driven {
            state_driven_n += 1;
            let on = EmissiveState {
                tendril: 1.0,
                eyes_glow: true,
            };
            let dark_on = shoot(&mut wr, gpu, &mut off, Some((0.0, on)))?;
            let (on_glow, _) = glow(&dark_on);
            if on_glow <= dark_glow {
                state_fail.push(format!("{kind:?} ({dark_glow} → {on_glow} px)"));
            }
        }
    }

    wr.destroy(gpu);
    off.destroy(gpu);

    c.record(
        "m1.an_active_layer_still_glows_at_world_light_zero",
        dark_fail.is_empty(),
        format!(
            "{expected_glowing} mob(s) have a layer active at vanilla's synched \
             defaults; {} went black instead{}",
            dark_fail.len(),
            if dark_fail.is_empty() { String::new() } else { format!(" ({})", dark_fail.join(", ")) }
        ),
    );
    c.record(
        "m2.every_other_mob_is_perfectly_black_at_light_zero",
        black_fail.is_empty(),
        format!(
            "{} of {checked} mobs graded as the control half; {} stayed lit{}",
            checked - expected_glowing,
            black_fail.len(),
            if black_fail.is_empty() { String::new() } else { format!(" ({})", black_fail.join(", ")) }
        ),
    );
    c.record(
        "m3.no_emissive_pixel_lands_outside_the_mobs_silhouette",
        stray_fail.is_empty(),
        format!(
            "a layer drifting off its model would paint here; {} did{}",
            stray_fail.len(),
            if stray_fail.is_empty() { String::new() } else { format!(" ({})", stray_fail.join(", ")) }
        ),
    );
    c.record(
        "m4.raising_the_entity_state_grows_the_glow",
        state_fail.is_empty() && state_driven_n > 0,
        format!(
            "{state_driven_n} state-driven mob(s) (warden tendrils, creaking eyes), \
             rendered at the sway's zero crossing so only the alpha can move the \
             count; {} did not grow{}",
            state_fail.len(),
            if state_fail.is_empty() { String::new() } else { format!(" ({})", state_fail.join(", ")) }
        ),
    );
    c.record(
        "m5.the_glow_count_matches_the_layer_table",
        glowing == expected_glowing && checked > 80,
        format!(
            "{glowing} of {checked} mobs glow in the dark; the decompiled layer \
             table predicts {expected_glowing}"
        ),
    );
    c.finish("EMISSIVE", EMISSIVE_WITNESSES)
}

// ---------------------------------------------------------------------------
// --etf-check: the random-entity-texture gate
// ---------------------------------------------------------------------------

const ETF_WITNESSES: usize = 8;

/// Flat colours the fixture pack paints its alternate textures, chosen to be
/// unmistakable against any vanilla mob texture and against each other.
const ETF_COLORS: [([u8; 3], &str); 3] =
    [([255, 0, 0], "red"), ([0, 255, 0], "green"), ([0, 0, 255], "blue")];

/// The ETF gate: **a pack's variant rules end up on the mob's pixels, and only
/// on the slot the rule names.**
///
/// There is no published OptiFine pack to grade against and no decompile to
/// transcribe (see `rewo_data::etf` — this is the one Rewo subsystem with no
/// ground truth), so this builds its own resource pack in a temp directory:
/// real `.properties` files, real PNGs, loaded through the same `etf::load_pack`
/// the live client uses. Flat primary colours make the assertion exact — variant
/// *n* must paint the mob colour *n*, which no partial success can fake.
///
/// Mutations run against it: making the UV offset ignore the quad's texture slot
/// fails `f5`; making the draw ignore its variant fails `f2`; pointing the
/// emissive overlay's UVs at a single transparent texel fails `f6`; making the
/// overlay fully opaque fails `f7`.
fn run_etf_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    _args: &MobshotArgs,
) -> Result<(), String> {
    let dir = std::env::temp_dir().join("rewo-etf-fixture");
    std::fs::create_dir_all(&dir).map_err(|e| format!("etf fixture dir: {e}"))?;
    let pack_path = dir.join("rewo-etf-fixture.zip");
    write_etf_fixture(&pack_path)?;
    let etf = rewo_data::etf::load_pack(&pack_path)?;
    println!("[mobshot] etf fixture: {}", pack_path.display());

    let mut c = Checker::new();
    c.record(
        "f1.the_fixture_packs_rules_load",
        etf.rules.len() == 2 && etf.textures.len() >= 4,
        format!(
            "{} texture(s) carry rules (want 2: cow + sheep_wool), {} alternate \
             image(s) packed",
            etf.rules.len(),
            etf.textures.len()
        ),
    );
    // A rule naming the *vanilla* texture (`textures.1=cow_temperate.png`) is
    // how packs give the original a share of the weighting. Dropping it as
    // "textureless" would hand that share to the alternates and make nearly
    // every cow a variant cow — invisible in any single screenshot, badly wrong
    // in aggregate.
    c.record(
        "f4.a_rule_naming_the_vanilla_texture_is_kept_not_dropped",
        etf.rules
            .get("cow")
            .is_some_and(|v| v.iter().filter(|r| r.texture.is_none()).count() == 1),
        format!(
            "cow rules: {:?}",
            etf.rules
                .get("cow")
                .map(|v| v.iter().map(|r| (r.index, r.texture.is_some())).collect::<Vec<_>>())
        ),
    );

    let (w, h) = (512u32, 512u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(
        gpu,
        crate::live_cmd::font_data(baked),
        crate::live_cmd::entity_textures_with(baked, &etf),
    )?;
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);

    // ---- the cow wears the variant it was given ---------------------------
    let kind = EntityModelKind::Cow;
    let (vp, right, up, eye) = frame_kind(&wr, kind).ok_or("the cow model is unavailable")?;
    wr.set_camera(eye.to_array());

    let mut shot = |wr: &mut WorldRenderer, gpu: &mut Gpu, off: &mut Offscreen, variant: u16| {
        let d = EntityDraw {
            variant,
            ..neutral_draw(kind)
        };
        wr.set_entities(std::slice::from_ref(&d), right, up, 0.0);
        off.render(gpu, Some((wr, vp)), &draw, BG)?;
        off.read_rgba(gpu)
    };
    let vanilla = shot(&mut wr, gpu, &mut off, 0)?;
    // The fixture's `textures.1` is the vanilla texture, which the loader
    // resolves to "no variant" — so nothing ever draws with id 1, and an id the
    // atlas has no slot for must fall back rather than sample garbage.
    let one = shot(&mut wr, gpu, &mut off, 1)?;
    let one_diff = one.chunks(4).zip(vanilla.chunks(4)).filter(|(a, b)| a != b).count();
    c.record(
        "f3.a_variant_id_with_no_atlas_slot_falls_back_to_vanilla",
        one == vanilla,
        format!("{one_diff} px differ from the no-variant render (want 0)"),
    );

    let mut colour_fail = Vec::new();
    let mut colour_detail = Vec::new();
    for (n, (rgb, name)) in ETF_COLORS.iter().enumerate() {
        let variant = n as u16 + 2;
        let img = shot(&mut wr, gpu, &mut off, variant)?;
        let (mut hits, mut mob) = (0usize, 0usize);
        for i in 0..img.len() / 4 {
            if img[i * 4..i * 4 + 3] == vanilla[i * 4..i * 4 + 3] {
                continue; // background (identical in both shots)
            }
            mob += 1;
            // The flat colour arrives shaded per face, so compare by dominant
            // channel rather than exact value.
            let px = &img[i * 4..i * 4 + 3];
            let brightest = px.iter().enumerate().max_by_key(|(_, c)| **c).map(|(c, _)| c);
            let want = rgb.iter().position(|c| *c == 255);
            if brightest == want && px.iter().max() >= Some(&24) {
                hits += 1;
            }
        }
        if mob == 0 {
            colour_fail.push(format!("variant {variant} never reached the draw"));
            continue;
        }
        let share = hits as f64 / mob as f64;
        colour_detail.push(format!("{name} {:.0}%", share * 100.0));
        if share < 0.9 {
            colour_fail.push(format!("variant {variant} ({name}) only {hits}/{mob} px"));
        }
    }
    c.record(
        "f2.each_variant_paints_the_mob_its_own_texture",
        colour_fail.is_empty(),
        format!(
            "3 alternates, by dominant channel over the mob's own pixels: {}{}",
            colour_detail.join(", "),
            if colour_fail.is_empty() { String::new() } else { format!(" — {}", colour_fail.join("; ")) }
        ),
    );

    // ---- a pack `_e.png` glows, and only where it is opaque ---------------
    // Rendered at world light 0, where the base model is black: the patch must
    // be lit and the rest of the pig must not be.
    let pig = EntityModelKind::Pig;
    let dark = EntityDraw {
        light: [0.0; 3],
        kind: pig,
        ..neutral_draw(pig)
    };
    wr.set_entities(std::slice::from_ref(&dark), right, up, 0.0);
    off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
    let pig_img = off.read_rgba(gpu)?;
    wr.set_entities(&[], right, up, 0.0);
    off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
    let bg = off.read_rgba(gpu)?;
    let (mut lit, mut body) = (0usize, 0usize);
    for i in 0..pig_img.len() / 4 {
        if pig_img[i * 4..i * 4 + 3] == bg[i * 4..i * 4 + 3] {
            continue;
        }
        body += 1;
        if pig_img[i * 4..i * 4 + 3].iter().any(|c| *c >= GLOW_LEVEL) {
            lit += 1;
        }
    }
    c.record(
        "f6.a_pack_emissive_overlay_glows_at_world_light_zero",
        etf.emissive.contains(&"pig") && lit > 0,
        format!(
            "pig_temperate_e.png picked up = {}, {lit}/{body} px lit at light 0",
            etf.emissive.contains(&"pig")
        ),
    );
    c.record(
        "f7.the_overlays_alpha_is_respected_not_ignored",
        lit > 0 && lit < body,
        format!(
            "the fixture overlay is opaque over one sixteenth of the sheet; \
             {lit}/{body} px of the pig lit. The bound is the mutation: an \
             ignored alpha lights every pixel the mob covers"
        ),
    );
    let plain = EntityDraw {
        light: [0.0; 3],
        kind: EntityModelKind::Cow,
        ..neutral_draw(EntityModelKind::Cow)
    };
    wr.set_entities(std::slice::from_ref(&plain), right, up, 0.0);
    off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
    let cow_img = off.read_rgba(gpu)?;
    let stray = (0..cow_img.len() / 4)
        .filter(|i| {
            cow_img[i * 4..i * 4 + 3] != bg[i * 4..i * 4 + 3]
                && cow_img[i * 4..i * 4 + 3].iter().any(|c| *c >= GLOW_LEVEL)
        })
        .count();
    c.record(
        "f8.a_mob_the_pack_gives_no_overlay_stays_black",
        stray == 0,
        format!("{stray} px stayed lit on the cow at light 0 (want 0)"),
    );

    // ---- a rule on one texture moves only that texture --------------------
    // The sheep has three (body, wool, undercoat since M68); a pack varying
    // the *wool* must leave both others at zero offset. A per-slot table
    // indexed by the wrong axis passes every property above and fails here —
    // and the third slot strengthens it, because the undercoat's quads are
    // geometric twins of the body's and would follow any offset keyed off
    // position rather than off the texture slot.
    let wool_slot = rewo_gpu::mobs::MOBS
        .iter()
        .find(|d| d.kind == EntityModelKind::Sheep)
        .and_then(|d| d.textures.iter().position(|t| *t == "sheep_wool"))
        .expect("the sheep lists a wool texture");
    let offsets = wr
        .entity_pass()
        .expect("entity pass")
        .variant_offsets(EntityModelKind::Sheep, 2)
        .map(|o| o.to_vec());
    let only_wool = offsets.as_ref().is_some_and(|o| {
        o.len() >= 2
            && o.iter()
                .enumerate()
                .all(|(i, v)| (*v != [0.0, 0.0]) == (i == wool_slot))
    });
    c.record(
        "f5.a_rule_on_one_texture_shifts_only_that_texture",
        only_wool,
        format!(
            "sheep per-slot offsets = {offsets:?}; the wool is slot {wool_slot} \
             and is the only one that moved"
        ),
    );

    wr.destroy(gpu);
    off.destroy(gpu);
    // The pack is left in place: `rewo live --pack` uses the same fixture for
    // an end-to-end look, and it costs a few KB in the temp dir.
    c.finish("ETF", ETF_WITNESSES)
}

/// Build the fixture resource pack: a cow with alternates (the vanilla texture
/// plus three flat colours), a sheep whose *wool* alone varies, a pig with an
/// emissive overlay, and one rule carrying an unevaluatable condition — which
/// must load without disturbing the others.
fn write_etf_fixture(path: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("etf fixture: {e}"))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
        use std::io::Write;
        zip.start_file(name, opts).map_err(|e| format!("etf fixture {name}: {e}"))?;
        zip.write_all(bytes).map_err(|e| format!("etf fixture {name}: {e}"))
    };

    // The cow: `textures.1` is the vanilla file, 2..4 the flat colours, and 5
    // carries a biome condition Rewo cannot evaluate (so it must never be
    // picked, and must not shift the others' weighting off their ids).
    let mut props = String::from("# rewo etf fixture\ntextures.1=cow_temperate.png\n");
    for (n, (_, name)) in ETF_COLORS.iter().enumerate() {
        props += &format!("textures.{}=rewo_{name}.png\n", n + 2);
    }
    props += "textures.5=rewo_red.png\nbiomes.5=swamp\n";
    put(
        "assets/minecraft/optifine/random/entity/cow/cow_temperate.properties",
        props.as_bytes(),
    )?;
    for (rgb, name) in ETF_COLORS {
        put(
            &format!("assets/minecraft/textures/entity/cow/rewo_{name}.png"),
            &flat_png(64, 64, rgb),
        )?;
    }
    // A pig with an emissive overlay: transparent except for a patch, which must
    // glow at world light 0 and must not tint the pig anywhere else.
    put(
        "assets/minecraft/textures/entity/pig/pig_temperate_e.png",
        &patch_png(64, 64, [0, 255, 255]),
    )?;
    // The sheep: only the wool varies (sheep_wool.png is 64x32).
    put(
        "assets/minecraft/optifine/random/entity/sheep/sheep_wool.properties",
        b"textures.2=rewo_wool.png\n",
    )?;
    put(
        "assets/minecraft/textures/entity/sheep/rewo_wool.png",
        &flat_png(64, 32, [255, 0, 0]),
    )?;
    zip.finish().map_err(|e| format!("etf fixture finish: {e}"))?;
    Ok(())
}

/// A transparent RGBA PNG with an opaque block in its top-left sixteenth — the
/// shape of an emissive overlay, which covers part of a texture.
///
/// A *quarter* was the first size tried, and it lit 87% of the rendered pig:
/// the top-left of a 64x64 mob sheet carries the head and most of the body, so
/// the fraction of the *sheet* an overlay covers is not the fraction of the
/// *model* it lights. A sixteenth leaves `f7` an unambiguous margin below the
/// "alpha ignored" case it is there to exclude.
fn patch_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let on = x < w / 4 && y < h / 4;
            px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], if on { 255 } else { 0 }]);
        }
    }
    encode_png(w, h, &px)
}

/// A flat opaque RGBA PNG.
fn flat_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    encode_png(w, h, &px)
}

fn encode_png(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(std::io::Cursor::new(&mut out), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(px).expect("png data");
    }
    out
}

// ---------------------------------------------------------------------------
// --tint-check: the dye gate
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// --variant-check: vanilla's metadata-driven texture variants (M64)
// ---------------------------------------------------------------------------

const VARIANT_WITNESSES: usize = 13;

/// The mobs whose texture a synched metadata field chooses, and the registry
/// name each carries in the `entity_types` table.
///
/// The last two are M68's: a tropical fish's packed variant picks the pattern
/// **layer**'s sheet, which is the same "one per-draw variant id addresses N
/// sheets" mechanism, applied to a mob's *second* texture slot rather than its
/// first. Two entries because one wire name has two meshes.
const VARIANT_MOBS: [(EntityModelKind, &str); 8] = [
    (EntityModelKind::Cat, "minecraft:cat"),
    (EntityModelKind::Wolf, "minecraft:wolf"),
    (EntityModelKind::Frog, "minecraft:frog"),
    (EntityModelKind::Axolotl, "minecraft:axolotl"),
    (EntityModelKind::Horse, "minecraft:horse"),
    (EntityModelKind::Llama, "minecraft:llama"),
    (EntityModelKind::TropicalFish, "minecraft:tropical_fish"),
    (EntityModelKind::TropicalFishLarge, "minecraft:tropical_fish"),
];

/// Every texture key a kind lists — M64's mobs vary their *first* slot, M68's
/// fish its second, so the atlas and render rows walk all of them.
fn kind_texture_keys(kind: EntityModelKind) -> &'static [&'static str] {
    rewo_gpu::mobs::MOBS
        .iter()
        .find(|d| d.kind == kind)
        .map_or(&[][..], |d| d.textures)
}

/// The variant gate: **the sheet the server names is the sheet that renders,
/// and nothing else on the mob moves.**
///
/// The subject is the production path throughout — `rewo_data`'s bake, the
/// shipped `EntityPass` atlas, `rewo_net`'s real `route_set_entity_data`, and
/// `live_cmd`'s own `vanilla_variant`. Nothing here assembles a parallel
/// mapping, which is M45's and M41's failure mode.
fn run_variant_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    _args: &MobshotArgs,
) -> Result<(), String> {
    use rewo_data::mob_variants as mv;
    let mut c = Checker::new();

    // ---- 1. the bake -------------------------------------------------------
    //
    // Every alternate must be its base's size, because it reuses the base's
    // UVs — the constraint M57b puts on a pack's alternates, and one vanilla
    // satisfies by construction. One that did not would render scrambled.
    let baked_ids: std::collections::HashMap<u16, (&str, u32, u32)> = baked
        .mob_variant_textures
        .iter()
        .map(|t| (t.index, (t.key, t.w, t.h)))
        .collect();
    let declared: Vec<(&str, u16, &str)> = mv::specs().collect();
    let missing: Vec<&str> = declared
        .iter()
        .filter(|(_, id, _)| !baked_ids.contains_key(id))
        .map(|(_, _, p)| *p)
        .collect();
    let wrong_size: Vec<&str> = declared
        .iter()
        .filter(|(k, id, _)| {
            baked_ids
                .get(id)
                .zip(rewo_data::assets::mob_texture_size(k))
                .is_some_and(|((_, w, h), (bw, bh))| (*w, *h) != (bw, bh))
        })
        .map(|(_, _, p)| *p)
        .collect();
    c.record(
        "n1.every_declared_variant_sheet_baked_at_its_bases_size",
        missing.is_empty() && wrong_size.is_empty() && declared.len() == 52,
        format!(
            "{} declared, {} baked, {} missing {missing:?}, {} the wrong size \
             {wrong_size:?}. Size is not a formality: a variant reuses its \
             base's UVs, so a differently-shaped sheet renders scrambled rather \
             than failing",
            declared.len(),
            baked.mob_variant_textures.len(),
            missing.len(),
            wrong_size.len()
        ),
    );

    // ---- 2. the ordinal tables --------------------------------------------
    //
    // Each of the three int-carrying mobs has its own out-of-bounds strategy,
    // and they genuinely differ. Asserted end-to-end — ordinal to atlas id —
    // rather than against the table this reads.
    let axo = |i: i32| mv::variant_id(mv::axolotl_texture(i));
    let llama = |i: i32| mv::variant_id(mv::llama_texture(i));
    let horse = |i: i32| mv::variant_id(mv::horse_texture(i));
    let distinct = |f: &dyn Fn(i32) -> Option<u16>, n: i32| {
        (0..n).map(|i| f(i)).collect::<std::collections::HashSet<_>>().len() == n as usize
    };
    c.record(
        "n2.each_ordinal_table_is_a_bijection_with_its_own_out_of_range_rule",
        distinct(&axo, 5)
            && distinct(&llama, 4)
            && distinct(&horse, 7)
            && axo(5) == axo(0)
            && axo(-1) == axo(0)
            && llama(9) == llama(3)
            && llama(-3) == llama(0)
            && horse(7) == horse(0)
            && horse(3 | (2 << 8)) == horse(3),
        format!(
            "axolotl 5 ids (ZERO: 5 -> {:?} = 0's {:?}), llama 4 (CLAMP: 9 -> \
             {:?} = 3's {:?}), horse 7 (WRAP: 7 -> {:?} = 0's {:?}). Three \
             different `ByIdMap` strategies, transcribed one by one — reading \
             any of them as a clamp would put a wrong coat on a horse at 7 and \
             a wrong axolotl at 5. The horse's markings live in the HIGH byte \
             and must not shift the coat: 0x203 -> {:?} = 3's {:?}",
            axo(5), axo(0), llama(9), llama(3), horse(7), horse(0),
            horse(3 | (2 << 8)), horse(3)
        ),
    );

    // ---- 3. the registry join ---------------------------------------------
    //
    // Cat/wolf/frog ids are the SERVER's, so the join is on the texture path.
    // Two registries with the same entries in opposite orders must resolve the
    // same cat to the same sheet.
    let cat_names = ["black", "jellie", "red"];
    let pack = |order: &[&str]| -> Vec<rewo_net::variant_parse::MobVariantDef> {
        let mut out = Vec::new();
        for n in order {
            let mut w = rewo_proto::writer::PacketWriter::default();
            w.string(&format!("minecraft:{n}")).bool(true);
            let mut body = w.buf;
            rewo_net::dimension_parse::builtin::write_network_nbt(
                &mut body,
                &rewo_proto::nbt::Nbt::Compound(vec![(
                    "asset_id".into(),
                    rewo_proto::nbt::Nbt::String(format!("minecraft:entity/cat/cat_{n}")),
                )]),
            );
            out.extend_from_slice(&body);
        }
        rewo_net::variant_parse::parse_single_asset_registry(
            &mut rewo_proto::reader::PacketReader::new(&out),
            order.len(),
        )
    };
    let fwd = pack(&cat_names);
    let mut rev_names = cat_names;
    rev_names.reverse();
    let rev = pack(&rev_names);
    let id_of = |defs: &[rewo_net::variant_parse::MobVariantDef], i: usize| {
        defs[i].texture(false).and_then(mv::variant_id)
    };
    c.record(
        "n3.the_registry_join_is_on_the_texture_not_on_the_id",
        id_of(&fwd, 0) == id_of(&rev, 2)
            && id_of(&fwd, 1) == id_of(&rev, 1)
            && id_of(&fwd, 0) != id_of(&fwd, 1),
        format!(
            "the same three cat variants registered in opposite orders resolve \
             black to {:?} either way and jellie to {:?}. cat/wolf/frog are \
             DATAPACK registries, so both the contents and the id order are the \
             server's; MUTATION keying the atlas on the wire id instead of the \
             texture path passes on a vanilla server and paints the wrong coat \
             on every cat the moment a datapack reorders the registry",
            id_of(&fwd, 0),
            id_of(&fwd, 1)
        ),
    );
    // The wolf's second input: `getTexture` picks `tame` over `wild`.
    let mut wbody = rewo_proto::writer::PacketWriter::default();
    wbody.string("minecraft:ashen").bool(true);
    let mut wb = wbody.buf;
    rewo_net::dimension_parse::builtin::write_network_nbt(
        &mut wb,
        &rewo_proto::nbt::Nbt::Compound(vec![(
            "assets".into(),
            rewo_proto::nbt::Nbt::Compound(vec![
                ("wild".into(), rewo_proto::nbt::Nbt::String("minecraft:entity/wolf/wolf_ashen".into())),
                ("tame".into(), rewo_proto::nbt::Nbt::String("minecraft:entity/wolf/wolf_ashen_tame".into())),
                ("angry".into(), rewo_proto::nbt::Nbt::String("minecraft:entity/wolf/wolf_ashen_angry".into())),
            ]),
        )]),
    );
    let wolves = rewo_net::variant_parse::parse_wolf_variant_registry(
        &mut rewo_proto::reader::PacketReader::new(&wb),
        1,
    );
    let (wild, tame) = (
        wolves[0].texture(false).and_then(mv::variant_id),
        wolves[0].texture(true).and_then(mv::variant_id),
    );
    c.record(
        "n4.a_tame_wolf_takes_a_different_sheet_from_a_wild_one",
        wild.is_some() && tame.is_some() && wild != tame && wolves[0].angry.is_some(),
        format!(
            "ashen wild -> {wild:?}, tame -> {tame:?}, and the entry's third \
             sheet ({:?}) is decoded but never chosen: `isAngry()` is \
             `remainingPersistentAngerTime > 0`, i.e. DATA_ANGER_END_TIME \
             (index 22, LONG) against the world clock — a texture that changes \
             with *time* rather than with a synched value, so it is recorded as \
             a gap rather than half-implemented",
            wolves[0].angry
        ),
    );

    // ---- 4. the atlas, and the render -------------------------------------
    let (w, h) = (512u32, 512u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    let r = (|| -> Result<(), String> {
        wr.init_entities(
            gpu,
            crate::live_cmd::font_data(baked),
            crate::live_cmd::entity_textures(baked),
        )?;
        let ring = OverlayRing::default();
        let draw = overlay_offscreen(&ring);
        let pass = wr.entity_pass().ok_or("no entity pass")?;
        // Every declared variant reached the atlas with an offset, and every
        // offset is distinct — two variants sharing a slot would render as one.
        let mut no_slot = Vec::new();
        let mut offsets: std::collections::HashSet<(EntityModelKind, Vec<[u32; 2]>)> =
            std::collections::HashSet::new();
        let mut collided = Vec::new();
        for (kind, _) in VARIANT_MOBS {
            for key in kind_texture_keys(kind) {
                for (k, id, path) in mv::specs().filter(|(k, _, _)| k == key) {
                    match pass.variant_offsets(kind, id) {
                        Some(o) => {
                            // Keyed by KIND: the offset is relative to that
                            // mob's own base slot, so two mobs can share one
                            // and mean different texels. M64 could use a
                            // global set because each of its six varied its
                            // only texture and no two bases sat 32 px apart;
                            // M68's two fish plans do (their pattern bases are
                            // adjacent 32x32 sheets and their alternates pack
                            // consecutively), so `tropical_a_pattern_6` and
                            // `tropical_b_pattern_2` land on the same relative
                            // offset while addressing different atlas slots.
                            let bits: Vec<[u32; 2]> =
                                o.iter().map(|v| [v[0].to_bits(), v[1].to_bits()]).collect();
                            if !offsets.insert((kind, bits)) {
                                collided.push(path);
                            }
                        }
                        None => no_slot.push((k, path)),
                    }
                }
            }
        }
        c.record(
            "n5.every_variant_has_its_own_atlas_slot",
            no_slot.is_empty() && collided.is_empty(),
            format!(
                "{} of {} declared variants reached the atlas with a per-slot UV \
                 offset distinct within their own mob; unpacked {no_slot:?}, \
                 collided {collided:?}. \
                 M64 grew the shelf region by 128 rows to fit them — before \
                 that, seven failed to pack and silently fell back to the base \
                 texture, which is a bug a render alone would not show",
                offsets.len(),
                declared.len()
            ),
        );

        // The render. Each mob at each of its variants must differ from its
        // base *and* from every other variant, and must move only its own
        // texture slot — a sheep's-wool-style containment check, generalized.
        let mut same_as_base = Vec::new();
        let mut dupes = Vec::new();
        for (kind, _) in VARIANT_MOBS {
            let Some((vp, right, up, eye)) = frame_kind(&wr, kind) else { continue };
            wr.set_camera(eye.to_array());
            let mut shot = |wr: &mut WorldRenderer, gpu: &mut Gpu, off: &mut Offscreen, v: u16| {
                let d = EntityDraw {
                    variant: v,
                    ..neutral_draw(kind)
                };
                wr.set_entities(std::slice::from_ref(&d), right, up, 0.0);
                off.render(gpu, Some((wr, vp)), &draw, BG)?;
                off.read_rgba(gpu)
            };
            let base = shot(&mut wr, gpu, &mut off, 0)?;
            let mut seen: Vec<(u16, Vec<u8>)> = Vec::new();
            for (_, id, path) in
                mv::specs().filter(|(k, _, _)| kind_texture_keys(kind).contains(k))
            {
                let img = shot(&mut wr, gpu, &mut off, id)?;
                if img == base {
                    same_as_base.push(path);
                }
                if let Some((_, other)) = seen.iter().find(|(_, o)| *o == img) {
                    let _ = other;
                    dupes.push(path);
                }
                seen.push((id, img));
            }
        }
        c.record(
            "n6.each_variant_renders_as_itself_and_not_as_its_neighbour",
            same_as_base.is_empty() && dupes.is_empty(),
            format!(
                "over all six mobs, {} variant(s) rendered identically to their \
                 base {same_as_base:?} and {} identically to another variant of \
                 the same mob {dupes:?}. Both halves are needed: an id with no \
                 slot silently falls back to the base (which is correct \
                 behaviour and would hide a missing bake), and two ids sharing \
                 a slot would render one coat for two variants",
                same_as_base.len(),
                dupes.len()
            ),
        );
        check_fish(&mut c, &mut wr, gpu, &mut off, &draw)?;
        Ok(())
    })();
    wr.destroy(gpu);
    off.destroy(gpu);
    r?;

    // ---- 5. the wire ------------------------------------------------------
    //
    // Driven through the real `route_set_entity_data`, so a slot that moves or
    // a kind gate that stops matching fails here rather than in a screenshot.
    check_variant_routing(&mut c)?;
    c.finish("VARIANT", VARIANT_WITNESSES)
}

/// M68's tropical fish: the three things a *variant id* cannot express.
///
/// n1/n5/n6 above already grade the pattern layer's twelve sheets, because a
/// pattern is an ordinary alternate on the fish's second texture slot. What is
/// left is what the packed int does *besides* naming a sheet: it picks the
/// mesh, and it carries two dye colours for two different layers.
fn check_fish(
    c: &mut Checker,
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    off: &mut Offscreen,
    draw: &OverlayDraw<'_>,
) -> Result<(), String> {
    use rewo_data::mob_variants as mv;
    // `TropicalFish.packVariant`, rebuilt from the decompiled formula so the
    // gate's input is not the unpacker's inverse.
    let pack = |base: i32, index: i32, body: i32, pattern: i32| -> i32 {
        ((base | (index << 8)) & 0xFFFF) | ((body & 0xFF) << 16) | ((pattern & 0xFF) << 24)
    };

    // ---- the shape is the LOW BIT -----------------------------------------
    //
    // Driven through `live_cmd::fish_kind`, the client's own resolver.
    let kind_of = |p: i32| crate::live_cmd::fish_kind(mv::FishVariant::unpack(p));
    let small = EntityModelKind::TropicalFish;
    let large = EntityModelKind::TropicalFishLarge;
    let shape_ok = kind_of(pack(0, 0, 0, 0)) == small
        && kind_of(pack(1, 0, 0, 0)) == large
        && kind_of(pack(0, 5, 15, 15)) == small
        && kind_of(pack(1, 5, 15, 15)) == large
        // KOB's whole packed id is 0, so a fish that has never synced is SMALL.
        && kind_of(0) == small
        // An undeclared pattern id is KOB — SMALL — not "whatever bit 0 says".
        && kind_of(pack(1, 6, 0, 0)) == small;
    c.record(
        "f1.the_fish_shape_is_the_packed_variants_low_bit",
        shape_ok,
        format!(
            "`Pattern.packedId` is `base.id | index << 8`, so SMALL/LARGE is \
             bit 0 and the pattern index is byte 1. MUTATION reading the shape \
             as a byte (`packed & 0xFF`) puts SUNSTREAK (0x100) on the large \
             mesh and STRIPEY (0x101) on it too, collapsing the distinction. \
             An undeclared id is KOB and therefore SMALL — {:?} — because \
             `Pattern.byId` is `ByIdMap.**sparse**`, not a clamp",
            kind_of(pack(1, 6, 0, 0))
        ),
    );

    // ---- the two meshes are different meshes -------------------------------
    let Some((vp_s, right_s, up_s, eye_s)) = frame_kind(wr, small) else {
        return Err("the small tropical fish model is unavailable".into());
    };
    let Some((vp_l, right_l, up_l, eye_l)) = frame_kind(wr, large) else {
        return Err("the large tropical fish model is unavailable".into());
    };
    let mut shot = |wr: &mut WorldRenderer,
                    gpu: &mut Gpu,
                    off: &mut Offscreen,
                    kind: EntityModelKind,
                    variant: u16,
                    dye: Option<[u8; 2]>| {
        let (vp, right, up, eye) = if kind == small {
            (vp_s, right_s, up_s, eye_s)
        } else {
            (vp_l, right_l, up_l, eye_l)
        };
        wr.set_camera(eye.to_array());
        let d = EntityDraw {
            variant,
            fish_dye: dye,
            // Broadside. `frame_kind` looks along -Z, and a tropical fish is
            // **2 model-px wide**: head-on, the only body face in view is a
            // 2x3 rect, and a pattern with no marks in that one rect measures
            // as *nothing at all* (`tropical_a_pattern_3` is exactly such a
            // sheet — an early build of this gate read it as a broken bake).
            // Yawed 90 degrees the flank faces the camera, which is where a
            // fish's pattern lives.
            yaw: 90.0,
            ..neutral_draw(kind)
        };
        wr.set_entities(std::slice::from_ref(&d), right, up, 0.0);
        off.render(gpu, Some((wr, vp)), draw, BG)?;
        off.read_rgba(gpu)
    };
    // Same camera framing for both would beg the question — `frame_kind` sizes
    // the camera to each model's own bbox — so compare the *meshes* instead.
    let quads = |k: EntityModelKind| {
        wr.entity_pass()
            .and_then(|p| p.neutral_quads(k))
            .map_or(0, |q| q.len())
    };
    let (qs, ql) = (quads(small), quads(large));
    let bottom_fin = ql > qs;
    c.record(
        "f2.the_large_plan_is_its_own_mesh_not_a_rescaled_small_one",
        qs > 0 && bottom_fin,
        format!(
            "small {qs} quads, large {ql}. `TropicalFishLargeModel` is a 2x6x6 \
             body against 2x3x6, a 5-deep tail against 6, fins a block higher, \
             and a **bottom_fin** the small plan has no part for at all — so \
             the counts must differ. MUTATION rendering both from one mesh \
             (the pre-M68 behaviour, which drew every fish as shape A) makes \
             them equal"
        ),
    );

    // ---- the two dyes land on two different layers -------------------------
    let px = |img: &[u8], i: usize| -> [u8; 3] { [img[i * 4], img[i * 4 + 1], img[i * 4 + 2]] };
    let set = |a: &[u8], b: &[u8]| -> Vec<usize> {
        (0..a.len() / 4).filter(|&i| px(a, i) != px(b, i)).collect()
    };
    let inter = |a: &[usize], b: &[usize]| -> Vec<usize> {
        a.iter().copied().filter(|i| b.binary_search(i).is_ok()).collect()
    };
    let pat = mv::fish_pattern_variant(mv::FishBase::Small, 3);
    let white = shot(wr, gpu, off, small, pat, Some([0, 0]))?;
    let body_set = set(&shot(wr, gpu, off, small, pat, Some([14, 0]))?, &white);
    let patt_set = set(&shot(wr, gpu, off, small, pat, Some([0, 14]))?, &white);
    let overlap = inter(&body_set, &patt_set).len();
    // The fish's own `neutral_draw` default: `None` is `DEFAULT_VARIANT`'s
    // WHITE/WHITE, not "untinted" — the sheep's t1, for the other table.
    let default_dye = shot(wr, gpu, off, small, pat, None)?;
    c.record(
        "f3.the_body_dye_and_the_pattern_dye_move_disjoint_layers",
        overlap == 0
            && !body_set.is_empty()
            && !patt_set.is_empty()
            && default_dye == white,
        format!(
            "colouring the body alone moves {} px, the pattern alone {} px, and \
             {overlap} px answer to both — `getModelTint` returns \
             `state.baseColor` while `TropicalFishPatternLayer` passes \
             `state.patternColor`, two layers and two fields. An unsynced fish \
             renders as `DEFAULT_VARIANT`'s WHITE/WHITE rather than untinted \
             ({}). MUTATION tinting both layers from one field makes the \
             overlap the whole set; MUTATION tinting only one leaves a set \
             empty. Which field is which is f3b's job, not this row's",
            body_set.len(),
            patt_set.len(),
            if default_dye == white { "byte-identical to dye [0,0]" } else { "DIFFERS" }
        ),
    );

    // ---- …and which of the two is the pattern -----------------------------
    //
    // A transposition of the two colour bytes would keep f3 green: the sets
    // just swap names. What labels them is the **variant**, which changes the
    // pattern layer's sheet and nothing else — so the pixels that move when
    // only the sheet changes must be pixels the *pattern* dye owns, and never
    // pixels the *body* dye owns under both sheets.
    //
    // Measured on the LARGE plan at GLITTER against BLOCKFISH: the two share
    // 40 differing opaque texels, where the small plan's most-overlapping pair
    // shares 14 and the pair f3 uses shares one. That difference is the whole
    // margin of the `bad == 0` half — a pair that barely overlaps would make
    // the transposed reading fail by a pixel or two rather than by a layer.
    let (p_a, p_b) = (
        mv::fish_pattern_variant(mv::FishBase::Large, 2),
        mv::fish_pattern_variant(mv::FishBase::Large, 3),
    );
    let l_white_a = shot(wr, gpu, off, large, p_a, Some([0, 0]))?;
    let l_white_b = shot(wr, gpu, off, large, p_b, Some([0, 0]))?;
    let sheet_change = set(&l_white_a, &l_white_b);
    let body_a = set(&shot(wr, gpu, off, large, p_a, Some([14, 0]))?, &l_white_a);
    let body_b = set(&shot(wr, gpu, off, large, p_b, Some([14, 0]))?, &l_white_b);
    let patt_a = set(&shot(wr, gpu, off, large, p_a, Some([0, 14]))?, &l_white_a);
    let patt_b = set(&shot(wr, gpu, off, large, p_b, Some([0, 14]))?, &l_white_b);
    let bad = inter(&sheet_change, &inter(&body_a, &body_b)).len();
    let good = inter(&sheet_change, &inter(&patt_a, &patt_b)).len();
    c.record(
        "f3b.the_pattern_dye_owns_the_pixels_the_pattern_sheet_owns",
        bad == 0 && good > 100,
        format!(
            "swapping only the pattern sheet moves {} px; {bad} of them are \
             pixels the *body* dye moves under both sheets, and {good} are \
             pixels the *pattern* dye moves under both. A pixel no pattern \
             covers renders identically either way, so the first number is \
             zero by construction — and MUTATION transposing bits 16..23 with \
             24..31 swaps the two roles, turning {bad} into {good}. This is \
             what pins body-against-pattern from pixels rather than from the \
             unpacker's own arithmetic, which f3 cannot do because a \
             transposition merely renames its two sets",
            sheet_change.len()
        ),
    );

    // ---- and the colour is the DIFFUSE table, not the sheep's --------------
    let lin = |v: u8| rewo_gpu::entities::srgb_to_linear(v as f32 / 255.0);
    let diffuse = rewo_gpu::mobs::DYE_DIFFUSE_COLORS;
    let wool = rewo_gpu::mobs::SHEEP_WOOL_COLORS;
    let mut fail = Vec::new();
    let mut wool_would_fail = 0usize;
    for dye in 1..16u8 {
        let img = shot(wr, gpu, off, small, pat, Some([dye, 0]))?;
        let want: [f32; 3] =
            std::array::from_fn(|c| lin(diffuse[dye as usize][c]) / lin(diffuse[0][c]));
        let alt: [f32; 3] =
            std::array::from_fn(|c| lin(wool[dye as usize][c]) / lin(wool[0][c]));
        let (mut sum, mut n) = ([0.0f64; 3], [0u32; 3]);
        for &i in &body_set {
            for c in 0..3 {
                let (a, b) = (white[i * 4 + c], img[i * 4 + c]);
                if a < 24 {
                    continue;
                }
                sum[c] += (lin(b) / lin(a)) as f64;
                n[c] += 1;
            }
        }
        for c in 0..3 {
            if n[c] == 0 {
                continue;
            }
            let got = (sum[c] / n[c] as f64) as f32;
            if (got - want[c]).abs() > want[c].max(0.02) * 0.08 {
                fail.push(format!("dye {dye} ch{c}: {got:.3} vs {:.3}", want[c]));
            }
            if (got - alt[c]).abs() > alt[c].max(0.02) * 0.08 {
                wool_would_fail += 1;
            }
        }
    }
    c.record(
        "f4.the_fish_dyes_are_getTextureDiffuseColor_not_the_sheeps_lerper",
        fail.is_empty() && wool_would_fail >= 40,
        format!(
            "15 dyes x 3 channels against `linear(diffuse[k])/linear(diffuse[0])`, \
             {} outside 8%{}. `TropicalFishRenderer.extractRenderState` calls \
             `getTextureDiffuseColor()` on both dyes with no `ColorLerper` in \
             sight, where the sheep's wool goes through \
             `ColorLerper.Type.SHEEP` — floor(x * 0.75) with WHITE overridden \
             to 0xE6E6E6. MUTATION using the wool table instead would put \
             {wool_would_fail} of 45 outside the same tolerance: the two \
             disagree sharply *against WHITE* even though a ratio between two \
             coloured dyes barely separates them at all",
            fail.len(),
            if fail.is_empty() { String::new() } else { format!(" ({})", fail.join(", ")) }
        ),
    );
    Ok(())
}

/// The metadata half of M64, through the production dispatcher.
///
/// Every index below was derived by counting `defineId` up the 26.2 hierarchy,
/// not from a wiki: `Entity` 0..7, `LivingEntity` 8..14, `Mob` 15,
/// `PathfinderMob` none, `AgeableMob` 16 **and** 17, `Animal` none,
/// `TamableAnimal` 18 and 19.
fn check_variant_routing(c: &mut Checker) -> Result<(), String> {
    use rewo_net::VariantKinds;
    let paths = rewo_data::DataPaths::for_version("26.2")
        .ok_or("no config dir for version data")?;
    let packets = rewo_data::packets::Packets::load(&paths.packets_json())?;
    let ids = rewo_net::ids::Ids::resolve(&packets)?;
    let etypes = rewo_data::entity_types::EntityTypes::load(&paths.registries_json())?;
    let tid = |n: &str| etypes.id_of(n).ok_or_else(|| format!("no {n}"));
    let kinds = VariantKinds {
        cat: etypes.id_of("minecraft:cat"),
        wolf: etypes.id_of("minecraft:wolf"),
        frog: etypes.id_of("minecraft:frog"),
        axolotl: etypes.id_of("minecraft:axolotl"),
        horse: etypes.id_of("minecraft:horse"),
        llama: etypes.id_of("minecraft:llama"),
        tropical_fish: etypes.id_of("minecraft:tropical_fish"),
    };
    let sheep = tid("minecraft:sheep")?;

    // eid, then (index, serializer, varint value), then the 0xFF terminator.
    let send = |type_id: i32, index: u8, ser: u8, value: u8, k: VariantKinds| {
        let mut t = rewo_world::entities::EntityTable::default();
        t.add(
            1,
            rewo_world::entities::EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0),
        );
        rewo_net::route_set_entity_data(
            ids.cb_play_set_entity_data,
            &[1u8, index, ser, value, 0xFF],
            &ids,
            &mut t,
            rewo_net::MetaKinds {
                sheep: Some(sheep),
                variant_kinds: k,
                ..Default::default()
            },
        );
        t
    };

    // Each mob's own (index, serializer), and the value landing.
    let rows: [(&str, Option<i32>, u8, u8); 6] = [
        ("cat", kinds.cat, 20, 21),
        ("wolf", kinds.wolf, 23, 25),
        ("frog", kinds.frog, 18, 27),
        ("axolotl", kinds.axolotl, 18, 1),
        ("horse", kinds.horse, 19, 1),
        ("llama", kinds.llama, 21, 1),
    ];
    let mut landed = Vec::new();
    let mut wrong_index = Vec::new();
    for (name, id, index, ser) in rows {
        let Some(id) = id else { continue };
        if send(id, index, ser, 3, kinds).variant(1) == Some(3) {
            landed.push(name);
        }
        // One slot either side must not: a neighbouring index is somebody
        // else's field, and reading it would give the mob a coat from a
        // number that means something else.
        for off in [index - 1, index + 1] {
            if send(id, off, ser, 3, kinds).variant(1).is_some() {
                wrong_index.push(format!("{name}@{off}"));
            }
        }
    }
    c.record(
        "n7.each_mobs_variant_arrives_at_its_own_index_and_nowhere_else",
        landed.len() == 6 && wrong_index.is_empty(),
        format!(
            "{landed:?} routed through the production dispatcher at cat 20 / \
             wolf 23 / frog 18 / axolotl 18 / horse 19 / llama 21; \
             {} neighbouring slot(s) also accepted {wrong_index:?}. Counted \
             `defineId` up the 26.2 hierarchy — Entity 0..7, LivingEntity \
             8..14, Mob 15, AgeableMob 16 AND 17, TamableAnimal 18 and 19 — \
             because this project has been bitten twice by a remembered index \
             (the sheep's wool is 18 not 17, and the player's customisation \
             mask is 16 in 26.2 where 1.21 put it at 17)",
            wrong_index.len()
        ),
    );

    // The shared slot. Index 18 BYTE is the sheep's wool byte *and* the
    // tamable flags byte, with the same serializer — only the kind separates
    // them, which is the M18 rule.
    let wolf = kinds.wolf.ok_or("no minecraft:wolf")?;
    let as_wolf = send(wolf, 18, 0, 0b0000_0100, kinds);
    let as_sheep = send(sheep, 18, 0, 0b0000_0100, kinds);
    c.record(
        "n8.index_18_byte_is_a_wool_byte_or_tamable_flags_by_kind_alone",
        as_wolf.is_tame(1)
            && !as_wolf.is_sheared(1)
            && as_wolf.wool_color(1) == Some(0)
            && !as_sheep.is_tame(1)
            && as_sheep.wool_color(1) == Some(4),
        format!(
            "the byte 0x04 makes a wolf tame ({}) and a sheep dye {:?}. Both \
             classes extend `Animal`, whose own accessor count is zero, so \
             `Sheep.DATA_WOOL_ID` and `TamableAnimal.DATA_FLAGS_ID` are the \
             same index AND the same serializer — the M18 rule, where only the \
             entity kind can tell them apart. MUTATION dropping the wolf's gate \
             and letting it fall through to the wool setter gives every tame \
             wolf dye 4 (yellow) and a fleece it does not have. The wolf's own              wool byte reads {:?} — untouched, which is the containment half",
            as_wolf.is_tame(1),
            as_sheep.wool_color(1),
            as_wolf.wool_color(1)
        ),
    );
    Ok(())
}

const TINT_WITNESSES: usize = 11;

/// The dye gate: **a dyed texture renders vanilla's colour, and nothing else on
/// the mob moves.**
///
/// Comparing pixels against the colour table directly would be defeated by the
/// per-face shade, so this compares *ratios*: vanilla multiplies the dye into
/// the vertex colour, so for any wool pixel
/// `linear(dyed) / linear(white) == linear(color[k]) / linear(color[0])`, per
/// channel, whatever the face shading or the geometry is. That prediction comes
/// from `SheepWoolLayer`'s semantics rather than from the renderer, and it holds
/// for all sixteen dyes at once.
///
/// The first version had a real flaw, and it took a mutation to find: it defined
/// "wool pixels" as those two dyes disagree on, which is derived from the
/// behaviour under test — a tint leaking onto every texture simply redefined the
/// whole sheep as wool and passed the containment check vacuously. The wool set
/// is now bounded against the silhouette, which is independent: vanilla's sheep
/// shows a bare face, four legs and hooves, so a correct tint can never cover
/// the whole mob. With that fixed, tinting every slot fails, and so does
/// dropping the sRGB linearize before the multiply (render discipline #1 —
/// caught numerically as a 0.805x ratio where vanilla's is 0.621x).
fn run_tint_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    _args: &MobshotArgs,
) -> Result<(), String> {
    let (w, h) = (512u32, 512u32);
    let mut off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(
        gpu,
        crate::live_cmd::font_data(baked),
        crate::live_cmd::entity_textures(baked),
    )?;
    let kind = EntityModelKind::Sheep;
    let (vp, right, up, eye) = frame_kind(&wr, kind).ok_or("the sheep model is unavailable")?;
    wr.set_camera(eye.to_array());
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    let mut c = Checker::new();

    // M68. Two knobs beyond M64's: whether the sheep is a baby, and an
    // override for the undercoat layer. Everything else resolves the layer
    // through the **client's own** `undercoat_visible`, so a rule graded here
    // is a rule `collect_entities` applies — a gate that set the flag by hand
    // would grade itself (M18's lesson, and M45's).
    let mut shot_ext = |wr: &mut WorldRenderer,
                        gpu: &mut Gpu,
                        off: &mut Offscreen,
                        dye: Option<u8>,
                        sheared: bool,
                        baby: bool,
                        force: Option<bool>| {
        let d = EntityDraw {
            dye,
            sheared,
            undercoat: force
                .unwrap_or_else(|| crate::live_cmd::undercoat_visible(kind, dye, baby)),
            mob: rewo_gpu::mobs::MobCombat {
                is_baby: baby,
                ..Default::default()
            },
            ..neutral_draw(kind)
        };
        wr.set_entities(std::slice::from_ref(&d), right, up, 0.0);
        off.render(gpu, Some((wr, vp)), &draw, BG)?;
        off.read_rgba(gpu)
    };
    // M64's rows measure the *fleece*, so they render with the second fleece
    // suppressed — see t6 for why that is now a necessary qualification and
    // not a convenience.
    let mut shot_of = |wr: &mut WorldRenderer,
                       gpu: &mut Gpu,
                       off: &mut Offscreen,
                       dye: Option<u8>,
                       sheared: bool| {
        let d = EntityDraw {
            dye,
            sheared,
            undercoat: false,
            ..neutral_draw(kind)
        };
        wr.set_entities(std::slice::from_ref(&d), right, up, 0.0);
        off.render(gpu, Some((wr, vp)), &draw, BG)?;
        off.read_rgba(gpu)
    };
    let mut shot = |wr: &mut WorldRenderer, gpu: &mut Gpu, off: &mut Offscreen, dye: Option<u8>| {
        shot_of(wr, gpu, off, dye, false)
    };
    // Vanilla's default wool colour is WHITE, not "untinted" — the layer tints
    // unconditionally, so a plain sheep's wool is 0xE6E6E6. An undyed draw must
    // therefore render exactly like dye 0.
    let white = shot(&mut wr, gpu, &mut off, Some(0))?;
    let undyed = shot(&mut wr, gpu, &mut off, None)?;
    let undyed_diff = undyed
        .chunks(4)
        .zip(white.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    c.record(
        "t1.an_undyed_sheep_renders_as_dyecolor_white",
        white == undyed,
        format!(
            "`None` is the mob's vanilla default dye, not \"no tint\": \
             {undyed_diff} px differ from dye 0 (want 0)"
        ),
    );

    let black = shot(&mut wr, gpu, &mut off, Some(15))?;
    // Wool pixels: those the two most extreme dyes disagree on. Anything else on
    // the sheep samples the untinted texture.
    let wool: Vec<usize> = (0..white.len() / 4)
        .filter(|i| white[i * 4..i * 4 + 3] != black[i * 4..i * 4 + 3])
        .collect();
    wr.set_entities(&[], right, up, 0.0);
    off.render(gpu, Some((&mut wr, vp)), &draw, BG)?;
    let bg = off.read_rgba(gpu)?;
    let silhouette = (0..white.len() / 4)
        .filter(|i| white[i * 4..i * 4 + 3] != bg[i * 4..i * 4 + 3])
        .count();
    let bare = silhouette.saturating_sub(wool.len());
    c.record(
        "t2.the_wool_set_is_bounded_by_an_independent_silhouette",
        wool.len() >= 400 && bare * 20 >= silhouette,
        format!(
            "{}/{silhouette} px are wool, {bare} bare — vanilla's sheep shows a \
             bare face, four legs and hooves, so a tint that reached every \
             texture would redefine the whole mob as wool and pass t4 vacuously",
            wool.len()
        ),
    );

    let lin = |c: u8| rewo_gpu::entities::srgb_to_linear(c as f32 / 255.0);
    let table = rewo_gpu::mobs::SHEEP_WOOL_COLORS;
    let mut ratio_fail = Vec::new();
    let mut stray_fail = Vec::new();
    for dye in 0..16u8 {
        let img = shot(&mut wr, gpu, &mut off, Some(dye))?;
        // Predicted per-channel ratio against the white reference.
        let want: [f32; 3] =
            std::array::from_fn(|c| lin(table[dye as usize][c]) / lin(table[0][c]));
        let mut sum = [0.0f64; 3];
        let mut n = [0u32; 3];
        for &i in &wool {
            for c in 0..3 {
                let (a, b) = (white[i * 4 + c], img[i * 4 + c]);
                // Skip near-black references: at 8 bits the ratio there is
                // mostly quantization.
                if a < 24 {
                    continue;
                }
                sum[c] += (lin(b) / lin(a)) as f64;
                n[c] += 1;
            }
        }
        for c in 0..3 {
            if n[c] == 0 {
                continue;
            }
            let got = (sum[c] / n[c] as f64) as f32;
            // 8% covers the 8-bit round trip at the dark end of the table.
            if (got - want[c]).abs() > want[c].max(0.02) * 0.08 {
                ratio_fail.push(format!("dye {dye} ch{c}: {got:.3}x vs {:.3}x", want[c]));
            }
        }
        // Containment: every non-wool pixel is untouched by the dye.
        let strayed = (0..img.len() / 4)
            .filter(|i| wool.binary_search(i).is_err() && img[i * 4..i * 4 + 3] != white[i * 4..i * 4 + 3])
            .count();
        if strayed > 0 {
            stray_fail.push(format!("dye {dye}: {strayed} px"));
        }
    }
    c.record(
        "t3.every_dye_renders_vanillas_wool_colour_as_a_ratio",
        ratio_fail.is_empty(),
        format!(
            "16 dyes x 3 channels against `linear(color[k])/linear(color[0])`, a \
             prediction from `SheepWoolLayer`'s semantics rather than from the \
             renderer; {} outside tolerance{}",
            ratio_fail.len(),
            if ratio_fail.is_empty() { String::new() } else { format!(" ({})", ratio_fail.join(", ")) }
        ),
    );
    c.record(
        "t4.no_dye_touches_a_pixel_outside_the_wool",
        stray_fail.is_empty(),
        format!(
            "the sheep's face and legs sample its *other* texture and must be \
             byte-identical across every dye; {} dye(s) moved them{}",
            stray_fail.len(),
            if stray_fail.is_empty() { String::new() } else { format!(" ({})", stray_fail.join(", ")) }
        ),
    );

    // --- M64: shearing ---------------------------------------------------
    //
    // `SheepWoolLayer.submit` opens `if (!state.isSheared)`, so shearing does
    // not recolour the fleece — the fur model is never submitted. The wool
    // pixel set computed above is independent of this (it is where two dyes
    // disagree), so it can grade the removal without being defined by it.
    let shorn = shot_of(&mut wr, gpu, &mut off, Some(0), true)?;
    fn px(img: &[u8], i: usize) -> &[u8] {
        &img[i * 4..i * 4 + 3]
    }
    let changed: Vec<usize> = (0..white.len() / 4)
        .filter(|&i| px(&shorn, i) != px(&white, i))
        .collect();
    let strayed = changed.iter().filter(|i| wool.binary_search(i).is_err()).count();
    let shorn_sil = (0..white.len() / 4)
        .filter(|&i| px(&shorn, i) != px(&bg, i))
        .count();
    c.record(
        "t5.shearing_removes_the_fleece_geometry_and_nothing_else",
        strayed == 0 && !changed.is_empty() && shorn_sil * 20 < silhouette * 19,
        format!(
            "{} px change, all inside the independently-derived wool set ({} \
             strayed), and the silhouette drops {silhouette} -> {shorn_sil}. \
             Both halves are needed: MUTATION ignoring the sheared bit changes \
             0 px, and MUTATION implementing it as \"skip the tint\" rather \
             than \"skip the layer\" leaves the silhouette exactly where it \
             was — the fleece is inflated 0.6/1.75/0.5 over the body, so a \
             shorn sheep is thinner, not merely a different colour",
            changed.len(),
            strayed
        ),
    );

    // And the other side of the same statement: with the fleece gone there is
    // nothing left *of it* for the dye to reach.
    //
    // M64 stated this without the qualification, because it believed the
    // fleece was the sheep's only tinted layer. It is not: `SheepWool-
    // UndercoatLayer` carries no `isSheared` test, so a real shorn dyed sheep
    // still answers the dye — which is u1 below. Both rows render with the
    // undercoat suppressed so this one keeps measuring what it always meant
    // to, the fleece.
    let shorn_black = shot_of(&mut wr, gpu, &mut off, Some(15), true)?;
    c.record(
        "t6.a_shorn_sheep_is_inert_to_the_dye_once_the_undercoat_is_suppressed",
        shorn == shorn_black && white != black,
        format!(
            "dyes 0 and 15 render byte-identically once the sheep is shorn and \
             the second fleece is held off, where on a woolly one they differ \
             over {} px — `SheepWoolLayer` is the only layer shearing stops. \
             MUTATION dropping the *tint* instead of the *geometry* passes \
             this row and fails t5; the two together pin which one happened",
            wool.len()
        ),
    );

    // --- M68: `SheepWoolUndercoatLayer` ----------------------------------
    //
    // A second fleece, drawn over the *body* mesh at `CubeDeformation.NONE`
    // and gated on `(isJebSheep || woolColor != WHITE) && !isBaby` — with no
    // `isSheared` test at all.
    let px = |img: &[u8], i: usize| -> [u8; 3] {
        [img[i * 4], img[i * 4 + 1], img[i * 4 + 2]]
    };
    let diff = |a: &[u8], b: &[u8]| -> Vec<usize> {
        (0..a.len() / 4).filter(|&i| px(a, i) != px(b, i)).collect()
    };
    // The reference every row below measures against: the same shorn sheep,
    // same dye, with the layer held off.
    let u_off = shot_ext(&mut wr, gpu, &mut off, Some(15), true, false, Some(false))?;
    let u_on = shot_ext(&mut wr, gpu, &mut off, Some(15), true, false, None)?;
    let coat = diff(&u_on, &u_off);
    let u_white = shot_ext(&mut wr, gpu, &mut off, Some(0), true, false, None)?;
    let u_white_off = shot_ext(&mut wr, gpu, &mut off, Some(0), true, false, Some(false))?;

    c.record(
        "u1.a_shorn_sheep_is_not_inert_to_the_dye_after_all",
        !coat.is_empty() && u_white == u_white_off,
        format!(
            "{} px of a shorn BLACK sheep are the undercoat, and a shorn WHITE \
             one is byte-identical to the same sheep with the layer held off. \
             This is the row that corrects t6's premise: `SheepWoolUndercoat- \
             Layer.submit` has no `isSheared` in it. MUTATION adding one — the \
             natural reading, and M64's — restores t6's original wording and \
             empties this set",
            coat.len()
        ),
    );

    let sil = |img: &[u8]| (0..img.len() / 4).filter(|&i| px(img, i) != px(&bg, i)).count();
    let (sil_off, sil_on) = (sil(&u_off), sil(&u_on));
    c.record(
        "u2.the_undercoat_is_the_body_mesh_so_the_silhouette_does_not_move",
        sil_on == sil_off && !coat.is_empty(),
        format!(
            "the shorn silhouette is {sil_off} px with the layer off and \
             {sil_on} with it on. `LayerDefinitions` maps SHEEP_WOOL_UNDERCOAT \
             to **sheepBodyLayer**, not the fur one, so its boxes are the \
             body's at deformation NONE — every one of them a texture-0 box \
             repeated. MUTATION building it from `SheepFurModel \
             .createFurLayer()` **and** leaving it in the solid range takes \
             the silhouette to 23522, out past the {} px the fleece adds in \
             t5. The second half of that mutation is not decoration: an \
             inflated layer in the *coplanar* range no longer sits at the \
             body's depth, so `EQUAL` rejects it and it vanishes instead of \
             growing — which u1 catches, not this row",
            silhouette - shorn_sil
        ),
    );

    // The gate's two suppressing terms, one at a time.
    let u_baby = shot_ext(&mut wr, gpu, &mut off, Some(15), true, true, None)?;
    let u_baby_off = shot_ext(&mut wr, gpu, &mut off, Some(15), true, true, Some(false))?;
    c.record(
        "u3.white_and_baby_each_suppress_the_layer_on_their_own",
        u_white == u_white_off && u_baby == u_baby_off && !coat.is_empty(),
        format!(
            "`(isJebSheep || woolColor != WHITE) && !isBaby`: a WHITE adult and \
             a BLACK baby both render byte-identically to themselves with the \
             layer held off, where a BLACK adult differs over {} px. MUTATION \
             dropping either term lights up the mob it must not — and the \
             third row is what stops both passing vacuously",
            coat.len()
        ),
    );

    // The absolute tint. The undercoat sheet is the wool region of `sheep.png`
    // cut out — all 467 of its opaque texels are byte-identical to the base
    // sheet at the same UV — and it sits on the base's own mesh, so the pixel
    // *behind* every undercoat pixel is the same texel at the same face shade.
    // Their quotient is therefore the tint itself, with no unknown constant:
    // this pins the table absolutely, where a ratio between two dyes could
    // not (the wool table is floor(diffuse * 0.75), and a uniform scale
    // cancels out of any such ratio).
    let mut abs_fail = Vec::new();
    for dye in 1..16u8 {
        let on = shot_ext(&mut wr, gpu, &mut off, Some(dye), true, false, None)?;
        let off_ref = shot_ext(&mut wr, gpu, &mut off, Some(dye), true, false, Some(false))?;
        // linear(SHEEP_WOOL_COLORS[k]) — the layer colour itself, which is
        // what the quotient below must equal.
        let want: [f32; 3] = std::array::from_fn(|c| lin(table[dye as usize][c]));
        let (mut sum, mut n) = ([0.0f64; 3], [0u32; 3]);
        for &i in &coat {
            for c in 0..3 {
                let (a, b) = (off_ref[i * 4 + c], on[i * 4 + c]);
                if a < 24 {
                    continue;
                }
                sum[c] += (lin(b) / lin(a)) as f64;
                n[c] += 1;
            }
        }
        for c in 0..3 {
            if n[c] == 0 {
                continue;
            }
            let got = (sum[c] / n[c] as f64) as f32;
            if (got - want[c]).abs() > want[c].max(0.02) * 0.08 {
                abs_fail.push(format!("dye {dye} ch{c}: {got:.3} vs {:.3}", want[c]));
            }
        }
    }
    c.record(
        "u4.the_undercoat_takes_the_fleeces_own_dye_table_absolutely",
        abs_fail.is_empty(),
        format!(
            "`SheepWoolUndercoatLayer` passes `state.getWoolColor()`, the same \
             call `SheepWoolLayer` makes, so the quotient of the layer against \
             the untinted body beneath it must be `linear(SHEEP_WOOL_COLORS[k])` \
             itself; {} of 15 dyes x 3 channels outside 8%{}. MUTATION tinting \
             it from `DYE_DIFFUSE_COLORS` (the tropical fish's table, and the \
             one the wool table is floor(x * 0.75) of) reads ~1.95x too bright \
             in linear — a difference no *ratio* between two dyes could see, \
             which is why this row is absolute",
            abs_fail.len(),
            if abs_fail.is_empty() { String::new() } else { format!(" ({})", abs_fail.join(", ")) }
        ),
    );

    // …and the fleece occludes it wherever the fleece actually covers it.
    //
    // Not *everywhere*: the fur boxes are shorter than the body's (a 6x6x6
    // head against 6x6x8, 4x6x4 legs against 4x12x4) and the sheet is
    // alpha-cutout, so a woolly sheep's snout and lower legs still show their
    // undercoat — as they do in vanilla. What must hold is that adding the
    // fleece can only ever *remove* undercoat pixels.
    let woolly_on = shot_ext(&mut wr, gpu, &mut off, Some(15), false, false, None)?;
    let woolly_off = shot_ext(&mut wr, gpu, &mut off, Some(15), false, false, Some(false))?;
    let woolly_coat = diff(&woolly_on, &woolly_off);
    let outside = woolly_coat.iter().filter(|i| coat.binary_search(i).is_err()).count();
    c.record(
        "u5.the_fleece_occludes_the_undercoat_wherever_it_covers_it",
        outside == 0 && woolly_coat.len() < coat.len() && !woolly_coat.is_empty(),
        format!(
            "the undercoat reaches {} px of a shorn sheep and only {} of a \
             woolly one, every one of them inside the shorn set ({outside} \
             outside). The layer leaves the solid range for a \
             `CompareOp::EQUAL`, no-write range drawn *after* it, so where the \
             inflated fleece has written a nearer depth the undercoat fails \
             the test, and where the fleece's cutout discarded it the body's \
             own depth stands and the undercoat passes — vanilla's `LEQUAL` \
             ordering read in reversed-Z. MUTATION giving that range `ALWAYS`, \
             or drawing it before the solid range, paints the undercoat over \
             the fleece and pushes pixels outside the shorn set",
            coat.len(),
            woolly_coat.len()
        ),
    );

    wr.destroy(gpu);
    off.destroy(gpu);
    c.finish("TINT", TINT_WITNESSES)
}
