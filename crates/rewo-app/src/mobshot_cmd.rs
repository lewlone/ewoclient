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
            let rgba = crate::skin_fetch::fetch_rgba64(&info.url)?;
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
    // The sheep has two (body, wool); a pack varying the wool must leave the
    // body's UVs at zero offset. A per-slot table indexed by the wrong axis
    // passes every property above and fails here.
    let offsets = wr
        .entity_pass()
        .expect("entity pass")
        .variant_offsets(EntityModelKind::Sheep, 2)
        .map(|o| o.to_vec());
    c.record(
        "f5.a_rule_on_one_texture_shifts_only_that_texture",
        offsets
            .as_ref()
            .is_some_and(|o| o.len() == 2 && o[0] == [0.0, 0.0] && o[1] != [0.0, 0.0]),
        format!("sheep per-slot offsets (body, wool) = {offsets:?}"),
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

const TINT_WITNESSES: usize = 4;

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

    let mut shot = |wr: &mut WorldRenderer, gpu: &mut Gpu, off: &mut Offscreen, dye: Option<u8>| {
        let d = EntityDraw {
            dye,
            ..neutral_draw(kind)
        };
        wr.set_entities(std::slice::from_ref(&d), right, up, 0.0);
        off.render(gpu, Some((wr, vp)), &draw, BG)?;
        off.read_rgba(gpu)
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

    wr.destroy(gpu);
    off.destroy(gpu);
    c.finish("TINT", TINT_WITNESSES)
}
