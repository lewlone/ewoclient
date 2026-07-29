//! `rewo healthbarshot --check` — the M59 floating-health-bar oracle.
//!
//! **This gate is different from every other one in Rewo, and the difference is
//! the point.** Fourteen milestones' worth of gates predict an answer from an
//! independent reading of the decompiled 26.2 source and assert the render
//! matches. That method does not work here: **vanilla renders no health bar
//! over any entity.** It shows the local player's hearts, a server-driven boss
//! bar and a horse's inventory screen, and nothing else — a floating bar over a
//! mob is a mod convention, and reading a mod's source to derive Rewo's design
//! is a licensing decision this project has deliberately avoided
//! (`REWO_FEATURE_SURVEY.md` §2).
//!
//! So the numbers were written down **first**, as a decision, in
//! [`REWO_HEALTH_BAR_SPEC.md`]. This file transcribes that spec independently —
//! the constants below are re-declared from the spec table, not imported from
//! `rewo_gpu` — and grades the render against it. Grading the render against
//! constants the render exports would be exactly the failure §0.0 collects:
//! M41's `t4` passed for months while the box was drawn in the wrong coordinate
//! space, because the witness had been written against the implementation.
//!
//! The path under test, end to end:
//!
//! ```text
//! raw update_attributes / set_entity_data bodies (built here)
//!   -> rewo_net::route_update_attributes / route_set_entity_data
//!   -> EntityTable::set_attribute / set_shared_flags / set_health
//!   -> live_cmd::resolve_health_bar        (the SAME resolver collect_entities uses)
//!   -> EntityPass::push_health_bar         (via oracle_health_bar — the emitter itself)
//!   -> EntityPass::set_draws               (the real frame path; text-range vertex count)
//! ```
//!
//! **Nothing here is measured by diffing two frames.** M50's control run — the
//! same scene rendered twice — differed in 41,284 pixels against a
//! 16,329-pixel signal, and M37 retracted a frame-diff witness for the same
//! reason. Every measurement below is a vertex position, a vertex colour or a
//! decoded value.
//!
//! **The absolute ruler is independent.** Font-pixel measurements divide world
//! distances by `TAG_PX * 8 / cell`, computed here from the baked font's own
//! cell size — not by asking the emitter how big a pixel is, and not by
//! normalising against the plate (which would make "the plate is 42 px wide"
//! true by construction).
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3, Vec4};

use rewo_data::assets;
use rewo_data::attributes::AttributeRegistry;
use rewo_data::entity_types::{EntityClasses, EntityTypes};
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_gpu::entities::{EntityDraw, EntityModelKind, HealthBar};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_world::entities::{EntityState, EntityTable};

/// Total named properties this gate asserts. Locked so a skipped property fails
/// the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 33;

// ---------------------------------------------------------------------------
// The spec, transcribed. `REWO_HEALTH_BAR_SPEC.md` § "The numbers".
//
// Re-declared rather than imported on purpose: these are what the render is
// graded against, and a gate that imports the implementation's constants
// asserts only that the implementation equals itself.
// ---------------------------------------------------------------------------

const SPEC_BAR_W: f32 = 40.0;
const SPEC_BAR_H: f32 = 3.0;
const SPEC_BAR_PAD: f32 = 1.0;
const SPEC_BAR_GAP: f32 = 2.0;
const SPEC_CRITICAL_FRAC: f32 = 0.25;
const SPEC_PLATE: [f32; 4] = [0.0, 0.0, 0.0, 0.25];
const SPEC_FILL_HEALTHY: [f32; 4] = [0.85, 0.20, 0.20, 1.0];
const SPEC_FILL_CRITICAL: [f32; 4] = [0.95, 0.55, 0.15, 1.0];
/// Spec split table, "below the line": `TAG_PX = 0.025`, annotated in
/// `entities.rs` as vanilla's nametag world scale per font pixel at cell 8.
const SPEC_TAG_PX: f32 = 0.025;
/// Spec split table: `TAG_LIFT = 0.4`, the tag anchor above the entity's head.
const SPEC_TAG_LIFT: f32 = 0.4;

/// A bar that shows is two quads — a plate and a fill — at six vertices each.
const BAR_VERTS: usize = 12;

/// World-space slop. The arithmetic is f32 through a scale of ~0.025, so this
/// is several orders above the last bit and several below any real error.
const EPS_PX: f32 = 1e-3;

const W: u32 = 256;
const H: u32 = 256;

#[derive(ClapArgs)]
pub struct HealthbarshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `eventshot`/`danceshot`/`attributeshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[healthbarshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: HealthbarshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[healthbarshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Graded against REWO_HEALTH_BAR_SPEC.md — there is no \
         vanilla health bar to transcribe."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_resolver(&mut c, &paths)?;
    check_emitter(&mut c, &baked)?;

    println!(
        "[healthbarshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
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
            "witness count {} != expected {EXPECTED_WITNESSES} — a named property \
             was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[healthbarshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// Wire bodies — built here, independent of any writer under test.
// ---------------------------------------------------------------------------

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut n = v as u32;
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

/// `ClientboundUpdateAttributesPacket`: VarInt entity, VarInt-counted snapshots
/// of (VarInt raw registry id, big-endian f64 base, VarInt-counted modifiers).
/// One bare base per attribute is all this gate needs — M55's `attributeshot`
/// owns the modifier arithmetic.
fn attrs_body(entity: i32, snaps: &[(i32, f64)]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(entity, &mut out);
    varint(snaps.len() as i32, &mut out);
    for (attr, base) in snaps {
        varint(*attr, &mut out);
        out.extend_from_slice(&base.to_be_bytes());
        varint(0, &mut out); // no modifiers
    }
    out
}

/// A `SynchedEntityData` delta stream: VarInt entity id (kept < 128 so it is
/// one byte), then `index u8 + serializer VarInt + value`, terminated by 0xFF.
fn meta_body(eid: u8, index: u8, serializer: u8, value: &[u8]) -> Vec<u8> {
    let mut b = vec![eid, index, serializer];
    b.extend_from_slice(value);
    b.push(0xFF);
    b
}

// ---------------------------------------------------------------------------
// f — the resolver: spec rules 4 and 5, and the gating below the line.
// ---------------------------------------------------------------------------

fn check_resolver(c: &mut Checker, paths: &DataPaths) -> Result<(), String> {
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let types = EntityTypes::load(&paths.registries_json())?;
    let classes = EntityClasses::resolve(&types)?;
    let reg = AttributeRegistry::load(&paths.registries_json())?;

    let zombie = types
        .id_of("minecraft:zombie")
        .ok_or("registries.json: no minecraft:zombie")?;
    let boat = types
        .id_of("minecraft:oak_boat")
        .ok_or("registries.json: no minecraft:oak_boat")?;
    let max_health = reg
        .id_of("max_health")
        .ok_or("registries.json: no minecraft:max_health")?;
    let ntd = reg
        .id_of("name_tag_distance")
        .ok_or("registries.json: no minecraft:name_tag_distance")?;

    println!(
        "[healthbarshot] ids: set_entity_data={} update_attributes={}; types: \
         zombie={zombie} boat={boat}; attrs: max_health={max_health} \
         name_tag_distance={ntd}",
        ids.cb_play_set_entity_data, ids.cb_play_update_attributes
    );

    // One helper per production seam, so no witness reaches around them.
    let send_attrs = |t: &mut EntityTable, body: &[u8]| {
        rewo_net::route_update_attributes(
            ids.cb_play_update_attributes,
            body,
            &ids,
            t,
            Some(&classes),
            Some(&types),
            Some(&reg),
        );
    };
    let send_meta = |t: &mut EntityTable, body: &[u8]| {
        rewo_net::route_set_entity_data(
            ids.cb_play_set_entity_data,
            body,
            &ids,
            t,
            rewo_net::MetaKinds {
                classes: Some(&classes),
                ..Default::default()
            },
        );
    };
    let spawn = |type_id: i32| {
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
        t
    };
    let resolve = |t: &EntityTable, type_name: &str, dist_sq: f64| {
        crate::live_cmd::resolve_health_bar(t, 1, Some(type_name), &reg, dist_sq)
    };

    // -- f1 / f2 / f3: spec rule 4, the fail-closed denominator ---------------

    // A zombie that has sent its health but no attributes at all. The supplier
    // says 20.0 and `Source::Default` says nobody has confirmed it.
    let mut t = spawn(zombie);
    send_meta(&mut t, &meta_body(1, 9, 3, &7.0f32.to_be_bytes()));
    let unsynced = resolve(&t, "minecraft:zombie", 4.0);
    c.record(
        "f1.no_synced_max_no_bar",
        unsynced.is_none(),
        format!(
            "{unsynced:?} (want None — spec rule 4; MUTATION PARTNER f2, which \
             differs only in having received an update_attributes. Accepting \
             Source::Default here is the 20.0 fallback the spec forbids and \
             would make f1 == f2)"
        ),
    );

    // The same entity, one real packet later.
    send_attrs(&mut t, &attrs_body(1, &[(max_health, 20.0)]));
    let synced = resolve(&t, "minecraft:zombie", 4.0);
    c.record(
        "f2.a_synced_max_draws_a_bar",
        synced == Some(HealthBar { current: 7.0, max: 20.0 }),
        format!("{synced:?} (want Some(7/20) — MUTATION PARTNER f1)"),
    );

    // The spec asks for this explicitly: the fail-closed None must be
    // distinguishable from "health is 1.0", which is `DATA_HEALTH_ID`'s own
    // synched default and therefore what an entity that never sent health reads
    // as. Both of these have health 1.0; only one has a denominator.
    let mut a = spawn(zombie); // never sent health, never sent attributes
    let mut b = spawn(zombie);
    send_attrs(&mut b, &attrs_body(1, &[(max_health, 20.0)]));
    let (ra, rb) = (
        resolve(&a, "minecraft:zombie", 4.0),
        resolve(&b, "minecraft:zombie", 4.0),
    );
    send_meta(&mut a, &meta_body(1, 9, 3, &1.0f32.to_be_bytes()));
    let ra_explicit = resolve(&a, "minecraft:zombie", 4.0);
    c.record(
        "f3.the_unsynced_case_is_not_health_one",
        ra.is_none()
            && ra_explicit.is_none()
            && rb == Some(HealthBar { current: 1.0, max: 20.0 }),
        format!(
            "no-attrs implicit={ra:?} no-attrs explicit-1.0={ra_explicit:?} \
             synced={rb:?} (the spec's 'distinguishably from health 1.0': Rewo \
             cannot tell a silent server from a 1 HP mob, so neither draws)"
        ),
    );

    // -- f4: spec rule 5, living entities only --------------------------------

    let mut boat_t = spawn(boat);
    send_attrs(&mut boat_t, &attrs_body(1, &[(max_health, 20.0)]));
    let boat_bar = crate::live_cmd::resolve_health_bar(
        &boat_t,
        1,
        Some("minecraft:oak_boat"),
        &reg,
        4.0,
    );
    c.record(
        "f4.a_non_living_entity_gets_no_bar",
        boat_bar.is_none(),
        format!(
            "{boat_bar:?} (want None — DefaultAttributes.SUPPLIERS has no boat, so \
             the supplier filter answers None even though a max_health packet was \
             accepted for it. MUTATION PARTNER f2, the same packet on a zombie)"
        ),
    );

    // -- f5 / f6: invisibility, through the real metadata path ----------------

    let mut inv = spawn(zombie);
    send_attrs(&mut inv, &attrs_body(1, &[(max_health, 20.0)]));
    send_meta(&mut inv, &meta_body(1, 9, 3, &7.0f32.to_be_bytes()));
    let before = resolve(&inv, "minecraft:zombie", 4.0);
    send_meta(&mut inv, &meta_body(1, 0, 0, &[0x20])); // FLAG_INVISIBLE = 5
    let hidden = resolve(&inv, "minecraft:zombie", 4.0);
    c.record(
        "f5.an_invisible_entity_gets_no_bar",
        before.is_some() && hidden.is_none(),
        format!(
            "visible={before:?} invisible={hidden:?} (Entity.DATA_SHARED_FLAGS_ID, \
             index 0 BYTE, FLAG_INVISIBLE = 5 -> 0x20. MUTATION PARTNER f6)"
        ),
    );
    send_meta(&mut inv, &meta_body(1, 0, 0, &[0x01])); // on fire, not invisible
    let unhidden = resolve(&inv, "minecraft:zombie", 4.0);
    c.record(
        "f6.clearing_the_flag_brings_the_bar_back",
        unhidden == before,
        format!(
            "{unhidden:?} (want {before:?} — bit 0 is FLAG_ONFIRE and must not \
             suppress; a gate that only ever sent 0x20 could not tell bit 5 from \
             'any flag at all')"
        ),
    );

    // -- f7 / f8 / f9: the name-tag distance ----------------------------------

    let mut d = spawn(zombie);
    send_attrs(&mut d, &attrs_body(1, &[(max_health, 20.0)]));
    let inside = resolve(&d, "minecraft:zombie", 63.99 * 63.99);
    let outside = resolve(&d, "minecraft:zombie", 64.01 * 64.01);
    c.record(
        "f7.beyond_the_name_tag_distance_no_bar",
        outside.is_none(),
        format!(
            "{outside:?} at 64.01 blocks (want None — EntityRenderer.extractNameTags \
             gates on distanceToCameraSq < Mth.square(nameTagDistance). MUTATION \
             PARTNER f8 at 63.99)"
        ),
    );
    c.record(
        "f8.just_inside_the_distance_a_bar",
        inside.is_some(),
        format!("{inside:?} at 63.99 blocks (MUTATION PARTNER f7)"),
    );
    // The default is 64, so a hard-coded 64 and the real attribute are
    // indistinguishable until the server moves it.
    send_attrs(&mut d, &attrs_body(1, &[(ntd, 128.0)]));
    let far = resolve(&d, "minecraft:zombie", 100.0 * 100.0);
    c.record(
        "f9.the_distance_is_the_attribute_not_a_constant",
        far.is_some(),
        format!(
            "{far:?} at 100 blocks after syncing name_tag_distance=128 (a hard-coded \
             64 would answer None here while passing f7 and f8 — that is the whole \
             point of this witness)"
        ),
    );

    // -- f10: id reuse --------------------------------------------------------

    let mut reuse = spawn(zombie);
    send_attrs(&mut reuse, &attrs_body(1, &[(max_health, 20.0)]));
    send_meta(&mut reuse, &meta_body(1, 0, 0, &[0x20]));
    reuse.remove(1);
    reuse.add(1, EntityState::new(0, zombie, 0.0, 0.0, 0.0, 0.0, 0.0));
    send_attrs(&mut reuse, &attrs_body(1, &[(max_health, 20.0)]));
    let recycled = resolve(&reuse, "minecraft:zombie", 4.0);
    c.record(
        "f10.a_recycled_id_does_not_inherit_invisibility",
        recycled.is_some(),
        format!(
            "{recycled:?} (want Some — remove() clears shared_flags; MUTATION \
             PARTNER f5, the same table before the remove)"
        ),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// The emitter's harness.
// ---------------------------------------------------------------------------

fn neutral_draw() -> EntityDraw<'static> {
    EntityDraw {
        pos: [0.0, 0.0, 0.0],
        width: 0.6,
        height: 1.8,
        color: [1.0, 1.0, 1.0],
        name: None,
        health: None,
        kind: EntityModelKind::Player,
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
        held: [None; 2],
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        light: [1.0, 1.0, 1.0],
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// One emitted bar, resolved back into **font pixels** and the entity-local
/// frame.
///
/// The vertex layout is the emitter's contract: six vertices of plate then six
/// of fill, each quad as `(p00, p10, p11, p00, p11, p01)` — the same pattern
/// `push_tag` has used since M2.
struct Measured {
    plate_w: f32,
    plate_h: f32,
    fill_w: f32,
    fill_h: f32,
    /// The fill's left edge inset from the plate's, along the camera right.
    inset_x: f32,
    /// The fill's bottom edge inset from the plate's, along the camera up.
    inset_y: f32,
    /// The plate's top edge relative to the tag anchor, in font pixels (0 with
    /// no nametag, negative below it).
    top: f32,
    plate_color: [f32; 4],
    fill_color: [f32; 4],
}

/// Recover [`Measured`] from emitted vertices. `scale` is derived by the caller
/// from `TAG_PX` and the font cell, never from the bar itself.
fn measure(
    verts: &[([f32; 3], [f32; 4])],
    anchor: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    scale: f32,
) -> Measured {
    // A hidden bar has nothing to measure. NaN rather than a panic so a
    // mutation that hides the bar reports through the witness list instead of
    // aborting the run before the other properties are printed.
    if verts.len() < BAR_VERTS {
        let nan = f32::NAN;
        return Measured {
            plate_w: nan,
            plate_h: nan,
            fill_w: nan,
            fill_h: nan,
            inset_x: nan,
            inset_y: nan,
            top: nan,
            plate_color: [nan; 4],
            fill_color: [nan; 4],
        };
    }
    let px = |p: [f32; 3]| {
        let d = sub(p, anchor);
        (dot3(d, right) / scale, dot3(d, up) / scale)
    };
    let (p00, p10, _p11, p01) = (verts[0].0, verts[1].0, verts[2].0, verts[5].0);
    let (f00, f10, _f11, f01) = (verts[6].0, verts[7].0, verts[8].0, verts[11].0);
    let (pl, pb) = px(p00);
    let (pr, _) = px(p10);
    let (_, pt) = px(p01);
    let (fl, fb) = px(f00);
    let (fr, _) = px(f10);
    let (_, ft) = px(f01);
    Measured {
        plate_w: pr - pl,
        plate_h: pt - pb,
        fill_w: fr - fl,
        fill_h: ft - fb,
        inset_x: fl - pl,
        inset_y: fb - pb,
        top: pt,
        plate_color: verts[0].1,
        fill_color: verts[6].1,
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS_PX
}

fn color_eq(a: [f32; 4], b: [f32; 4]) -> bool {
    (0..4).all(|i| (a[i] - b[i]).abs() <= 1e-6)
}

fn project(vp: &Mat4, p: [f32; 3], w: f32, h: f32) -> (f32, f32) {
    let c = *vp * Vec4::new(p[0], p[1], p[2], 1.0);
    (
        (c.x / c.w * 0.5 + 0.5) * w,
        (0.5 - c.y / c.w * 0.5) * h,
    )
}

// ---------------------------------------------------------------------------
// a..e — the emitter, against the spec.
// ---------------------------------------------------------------------------

fn check_emitter(c: &mut Checker, baked: &assets::BakedAssets) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    let font = crate::live_cmd::font_data(baked);
    // Fail closed rather than substituting a cell: without the baked font there
    // is no white texel, `has_font` is false, and the emitter draws nothing —
    // so a run without it would silently measure zeros.
    let cell = font
        .as_ref()
        .ok_or("no baked font — the bar shares the nametag's white texel")?
        .cell as f32;
    wr.init_entities(&mut gpu, font, crate::live_cmd::entity_textures(baked))?;

    // The independent ruler: font px -> world, as `push_tag` has scaled a
    // nametag since M2. Recomputed here from the spec's TAG_PX and the baked
    // font's own cell, so no measurement below asks the emitter how big a pixel
    // is.
    let scale = SPEC_TAG_PX * (8.0 / cell);
    println!("[healthbarshot] font cell {cell} px -> world scale {scale} per font px");

    let d0 = neutral_draw();
    let anchor = [d0.pos[0], d0.pos[1] + d0.height + SPEC_TAG_LIFT, d0.pos[2]];
    // Straight-on camera: yaw 0 looks along -Z, so `right` is -X and `up` is +Y.
    let (right, up) = crate::live_cmd::camera_basis(0.0, 0.0);

    // Emit one bar through the production emitter.
    let emit = |wr: &WorldRenderer, health: Option<HealthBar>, name: Option<&'static str>| {
        let mut d = neutral_draw();
        d.health = health;
        d.name = name;
        wr.entity_pass()
            .expect("entity pass")
            .oracle_health_bar(&d, right, up)
    };
    let hb = |current: f32, max: f32| Some(HealthBar { current, max });

    // -- a: the arithmetic ----------------------------------------------------

    let v = emit(&wr, hb(7.0, 20.0), None);
    c.record(
        "a0.a_shown_bar_is_two_quads",
        v.len() == BAR_VERTS,
        format!("{} vertices (want {BAR_VERTS} — a plate and a fill)", v.len()),
    );
    let m = measure(&v, anchor, right, up, scale);
    c.record(
        "a1.fill_width_is_the_exact_fraction",
        near(m.fill_w, 0.35 * SPEC_BAR_W),
        format!(
            "{:.6} px (want {:.6} = 7/20 * BAR_W). MUTATION PARTNER a2",
            m.fill_w,
            0.35 * SPEC_BAR_W
        ),
    );
    // Both plausible mis-wirings of the division produce a ratio >= 1, which
    // rule 3 hides — so either one shows up as a *missing* bar, not a wrong
    // width. That is worth stating: the failure mode is loud, not subtle.
    let swapped = emit(&wr, hb(20.0, 7.0), None);
    let default_denom = emit(&wr, hb(7.0, 1.0), None);
    c.record(
        "a2.the_division_is_health_over_max",
        swapped.is_empty() && default_denom.is_empty(),
        format!(
            "swapped(20/7)={} verts, metadata-default denominator(7/1)={} verts \
             (want 0 and 0 — both exceed full and rule 3 hides them, so neither \
             mis-wiring could ever produce a1's 14 px)",
            swapped.len(),
            default_denom.len()
        ),
    );
    c.record(
        "a3.plate_is_the_fill_grown_by_bar_pad",
        near(m.plate_w, SPEC_BAR_W + 2.0 * SPEC_BAR_PAD)
            && near(m.plate_h, SPEC_BAR_H + 2.0 * SPEC_BAR_PAD)
            && near(m.fill_h, SPEC_BAR_H)
            && near(m.inset_x, SPEC_BAR_PAD)
            && near(m.inset_y, SPEC_BAR_PAD),
        format!(
            "plate {:.4}x{:.4}, fill height {:.4}, inset ({:.4}, {:.4}) px (want \
             {}x{}, {}, ({}, {}) — dropping BAR_PAD is the mutation, and it moves \
             all five)",
            m.plate_w,
            m.plate_h,
            m.fill_h,
            m.inset_x,
            m.inset_y,
            SPEC_BAR_W + 2.0 * SPEC_BAR_PAD,
            SPEC_BAR_H + 2.0 * SPEC_BAR_PAD,
            SPEC_BAR_H,
            SPEC_BAR_PAD,
            SPEC_BAR_PAD
        ),
    );
    // Spec rule 2: `fraction * BAR_W` **exactly**, no rounding to whole pixels.
    let third = measure(&emit(&wr, hb(1.0, 3.0), None), anchor, right, up, scale);
    c.record(
        "a4.the_fill_is_not_rounded_to_whole_pixels",
        near(third.fill_w, SPEC_BAR_W / 3.0) && (third.fill_w - 13.0).abs() > 0.3,
        format!(
            "{:.6} px at 1/3 health (want {:.6}; a whole-pixel round would give \
             13.0 or 14.0 — the MUTATION PARTNER is any `.round()` on the width)",
            third.fill_w,
            SPEC_BAR_W / 3.0
        ),
    );

    // -- b: monotonicity and the clamps ---------------------------------------

    let sweep: Vec<Option<f32>> = (0..=20)
        .map(|i| {
            let v = emit(&wr, hb(i as f32, 20.0), None);
            (!v.is_empty()).then(|| measure(&v, anchor, right, up, scale).fill_w)
        })
        .collect();
    let visible: Vec<f32> = sweep.iter().flatten().copied().collect();
    c.record(
        "b1.width_is_non_decreasing_over_the_sweep",
        visible.len() == 20 && visible.windows(2).all(|w| w[1] >= w[0]),
        format!(
            "{} of 21 samples visible, non-decreasing = {} (health 20 is hidden by \
             rule 3, which is why it is 20 and not 21)",
            visible.len(),
            visible.windows(2).all(|w| w[1] >= w[0])
        ),
    );
    c.record(
        "b2.zero_health_is_exactly_zero_width",
        sweep[0] == Some(0.0),
        format!(
            "{:?} px (want Some(0.0) — an off-by-one in the fill's left edge shows \
             here as a non-zero stub; MUTATION PARTNER b1's monotone rise)",
            sweep[0]
        ),
    );
    // The spec's monotonicity row asks for "exactly BAR_W at max". It is not
    // reachable: rule 3 hides the bar at `fraction >= 1`, so BAR_W is the
    // fill's *supremum* and never an emitted value. The strongest observable
    // statement is the last visible sample, which is exact.
    c.record(
        "b3.bar_w_is_a_supremum_the_last_visible_sample_is_exact",
        sweep[19].is_some_and(|w| near(w, 0.95 * SPEC_BAR_W)) && sweep[20].is_none(),
        format!(
            "19/20 -> {:?} px (want {:.4}), 20/20 -> {:?} (want None). Recorded \
             deviation: the spec's 'exactly BAR_W at max' is unobservable because \
             rule 3 hides that case",
            sweep[19],
            0.95 * SPEC_BAR_W,
            sweep[20]
        ),
    );
    let negative = emit(&wr, hb(-5.0, 20.0), None);
    let neg_m = measure(&negative, anchor, right, up, scale);
    c.record(
        "b4.negative_health_clamps_to_empty_inside_the_plate",
        negative.len() == BAR_VERTS && near(neg_m.fill_w, 0.0) && neg_m.inset_x >= 0.0,
        format!(
            "fill {:.4} px at inset {:.4} (want 0.0 at +{} — MUTATION PARTNER is \
             the unclamped division, whose -0.25 fraction gives a -10 px fill \
             escaping the plate to the left)",
            neg_m.fill_w, neg_m.inset_x, SPEC_BAR_PAD
        ),
    );
    let over = emit(&wr, hb(30.0, 20.0), None);
    c.record(
        "b5.health_above_max_is_hidden_exactly_as_full_is",
        over.is_empty(),
        format!(
            "{} verts at 30/20 (want 0). The upper clamp is not independently \
             observable — rule 3 hides everything at or above full either way — so \
             this grades rules 1 and 3 composed, and the clamp stays as spec'd \
             defensive code",
            over.len()
        ),
    );

    // -- c: hidden at full (rule 3) -------------------------------------------

    let full = emit(&wr, hb(20.0, 20.0), None);
    let hair = emit(&wr, hb(19.999, 20.0), None);
    c.record(
        "c1.exactly_full_emits_zero_vertices",
        full.is_empty(),
        format!(
            "{} verts (want 0 — spec rule 3: absence IS the signal. MUTATION \
             PARTNER c2, a hair below full)",
            full.len()
        ),
    );
    c.record(
        "c2.a_hair_below_full_emits_the_bar",
        hair.len() == BAR_VERTS,
        format!(
            "{} verts at 19.999/20 (want {BAR_VERTS} — proves c1's zero is the rule \
             and not a broken emitter)",
            hair.len()
        ),
    );

    // -- d: the colour threshold ----------------------------------------------

    let at = measure(&emit(&wr, hb(5.0, 20.0), None), anchor, right, up, scale);
    let below = measure(&emit(&wr, hb(4.99, 20.0), None), anchor, right, up, scale);
    c.record(
        "d1.at_the_threshold_the_fill_is_healthy",
        color_eq(at.fill_color, SPEC_FILL_HEALTHY),
        format!(
            "{:?} at fraction exactly {SPEC_CRITICAL_FRAC} (want {SPEC_FILL_HEALTHY:?} \
             — 'below CRITICAL_FRAC' is strict; MUTATION PARTNER d2, and a `<=` \
             flip fails exactly this one)",
            at.fill_color
        ),
    );
    c.record(
        "d2.just_below_the_threshold_the_fill_is_critical",
        color_eq(below.fill_color, SPEC_FILL_CRITICAL),
        format!(
            "{:?} at fraction 0.2495 (want {SPEC_FILL_CRITICAL:?} — MUTATION \
             PARTNER d1)",
            below.fill_color
        ),
    );
    c.record(
        "d3.the_plate_is_the_nametag_plate",
        color_eq(at.plate_color, SPEC_PLATE),
        format!(
            "{:?} (want {SPEC_PLATE:?} — identical to push_tag's plate by spec, so \
             the two surfaces can never drift apart)",
            at.plate_color
        ),
    );

    // -- e: billboarding, anchoring, and the gap ------------------------------

    // Four cameras orbiting the entity at a fixed radius. Each uses the
    // production `camera_basis`, so the basis under test is the one the live
    // path hands `set_draws`.
    const R: f32 = 6.0;
    let target = Vec3::new(anchor[0], anchor[1], anchor[2]);
    let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
        60f32.to_radians(),
        W as f32 / H as f32,
        0.05,
    ));
    let mut widths = Vec::new();
    let mut fixed_widths = Vec::new();
    for az in [0.0f32, 90.0, 180.0, 270.0] {
        let (r, u) = crate::live_cmd::camera_basis(az, 0.0);
        let dir = Vec3::new(-az.to_radians().sin(), 0.0, az.to_radians().cos());
        let eye = target - dir * R;
        let vp = proj * Mat4::look_to_rh(eye, dir, Vec3::Y);
        let mut d = neutral_draw();
        d.health = hb(7.0, 20.0);
        let pass = wr.entity_pass().expect("entity pass");
        // NaN rather than an index panic if the bar is hidden, for the same
        // reason `measure` does it: a mutation should be *reported*.
        let span = |v: &[([f32; 3], [f32; 4])]| {
            if v.len() < BAR_VERTS {
                return f32::NAN;
            }
            let a = project(&vp, v[0].0, W as f32, H as f32);
            let b = project(&vp, v[1].0, W as f32, H as f32);
            b.0 - a.0
        };
        widths.push(span(&pass.oracle_health_bar(&d, r, u)));
        // The mutation, computed here rather than by editing the emitter: a
        // world-fixed basis.
        fixed_widths.push(span(&pass.oracle_health_bar(
            &d,
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
        )));
    }
    let spread = widths
        .iter()
        .fold(0.0f32, |acc, w| acc.max((w - widths[0]).abs()));
    c.record(
        "e1.four_azimuths_give_the_same_projected_width",
        spread <= 1.0 && widths[0].abs() > 8.0,
        format!(
            "{:?} px, spread {spread:.4} (want <= 1 px and a real width — MUTATION \
             PARTNER e2)",
            widths.iter().map(|w| (w * 100.0).round() / 100.0).collect::<Vec<_>>()
        ),
    );
    let collapsed = (1..4)
        .filter(|i| (fixed_widths[*i] - fixed_widths[0]).abs() > 1.0)
        .count();
    c.record(
        "e2.a_world_fixed_basis_collapses_three_of_the_four",
        collapsed == 3,
        format!(
            "world-fixed widths {:?} — {collapsed} of the other 3 differ from \
             azimuth 0 by more than a pixel (want 3: the two side views go edge-on \
             and the rear view mirrors). This is why e1 is a real property and not \
             an identity",
            fixed_widths.iter().map(|w| (w * 100.0).round() / 100.0).collect::<Vec<_>>()
        ),
    );
    // Anchoring: the bar rides the entity, and it rides the *head*.
    let base = emit(&wr, hb(7.0, 20.0), None);
    let mut moved_d = neutral_draw();
    moved_d.health = hb(7.0, 20.0);
    moved_d.pos = [3.0, -2.0, 5.0];
    let moved = wr
        .entity_pass()
        .expect("entity pass")
        .oracle_health_bar(&moved_d, right, up);
    // The length check is load-bearing, not defensive: `zip` over two empty
    // vectors makes `all` vacuously true, so without it a mutation that hides
    // the bar entirely would *pass* this witness. The `swapped_division`
    // mutation run found exactly that.
    let ok_move = base.len() == BAR_VERTS
        && moved.len() == BAR_VERTS
        && base.iter().zip(&moved).all(|(a, b)| {
            near(b.0[0] - a.0[0], 3.0) && near(b.0[1] - a.0[1], -2.0) && near(b.0[2] - a.0[2], 5.0)
        });
    c.record(
        "e3.the_bar_tracks_the_entity_position",
        ok_move,
        format!(
            "all {} vertices translate by exactly the entity delta = {ok_move} \
             (MUTATION PARTNER: any world-fixed anchor, which would not move at all)",
            base.len()
        ),
    );
    let mut tall_d = neutral_draw();
    tall_d.health = hb(7.0, 20.0);
    tall_d.height = neutral_draw().height + 1.25;
    let tall = wr
        .entity_pass()
        .expect("entity pass")
        .oracle_health_bar(&tall_d, right, up);
    let ok_head = base.len() == BAR_VERTS
        && tall.len() == BAR_VERTS
        && base
            .iter()
            .zip(&tall)
            .all(|(a, b)| near(b.0[1] - a.0[1], 1.25) && near(b.0[0], a.0[0]));
    c.record(
        "e4.the_bar_tracks_the_head_not_the_feet",
        ok_head,
        format!(
            "a +1.25 taller entity lifts every bar vertex by exactly 1.25 = \
             {ok_head} (the anchor is pos.y + height + TAG_LIFT; MUTATION PARTNER: \
             anchoring at pos.y, which would not move)"
        ),
    );
    // The gap: a nametag pushes the bar down by BAR_GAP, and only then.
    let no_tag = measure(&base, anchor, right, up, scale);
    let tagged = measure(
        &emit(&wr, hb(7.0, 20.0), Some("Zombie")),
        anchor,
        right,
        up,
        scale,
    );
    c.record(
        "e5.the_bar_hangs_at_the_anchor_with_no_tag",
        near(no_tag.top, 0.0),
        format!(
            "plate top {:.4} px from the anchor (want 0.0 — spec: 'at the anchor \
             itself when not'. MUTATION PARTNER e6)",
            no_tag.top
        ),
    );
    c.record(
        "e6.a_nametag_pushes_the_bar_down_by_bar_gap",
        near(no_tag.top - tagged.top, SPEC_BAR_GAP),
        format!(
            "plate top {:.4} -> {:.4} px, a drop of {:.4} (want {SPEC_BAR_GAP} — \
             MUTATION PARTNER e5; a bar that ignored d.name would show 0.0 here)",
            no_tag.top,
            tagged.top,
            no_tag.top - tagged.top
        ),
    );

    // -- g: the wiring. M45 and M41 both shipped gates that had quietly stopped
    // testing their subject, so this one proves `set_draws` really calls the
    // emitter rather than the emitter merely being callable.
    let mut with = neutral_draw();
    with.health = hb(7.0, 20.0);
    wr.set_entities(&[with], right, up, 0.0);
    let n_bar = wr.entity_pass().expect("entity pass").text_vert_count();
    wr.set_entities(&[neutral_draw()], right, up, 0.0);
    let n_none = wr.entity_pass().expect("entity pass").text_vert_count();
    c.record(
        "g1.set_draws_emits_the_bar_into_the_text_range",
        n_bar == BAR_VERTS as u32 && n_none == 0,
        format!(
            "text range: {n_bar} verts with a bar, {n_none} without (want \
             {BAR_VERTS} and 0 — the text range is the alpha-blended, \
             depth-write-free one nametags use, not the solid range world-space \
             sign text takes)"
        ),
    );
    let mut both = neutral_draw();
    both.health = hb(7.0, 20.0);
    both.name = Some("Zombie");
    wr.set_entities(&[both], right, up, 0.0);
    let n_both = wr.entity_pass().expect("entity pass").text_vert_count();
    let mut tag_only = neutral_draw();
    tag_only.name = Some("Zombie");
    wr.set_entities(&[tag_only], right, up, 0.0);
    let n_tag = wr.entity_pass().expect("entity pass").text_vert_count();
    c.record(
        "g2.a_tag_and_a_bar_coexist_in_the_text_range",
        n_tag > 0 && n_both == n_tag + BAR_VERTS as u32,
        format!(
            "tag alone {n_tag}, tag + bar {n_both} (want a difference of exactly \
             {BAR_VERTS} — the bar adds to the tag rather than replacing it)"
        ),
    );

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
