//! `rewo capeshot --check` — the vanilla player cape's oracle (M60).
//!
//! Serverless, validation-required, fail-closed. It grades four layers:
//!
//! 1. **The geometry and the rotation**, on the CPU, through the shipped
//!    [`cape_transform`] / [`cape_face_uv`] — the same functions `emit_cape`
//!    calls, so the oracle cannot grade a second copy of them.
//! 2. **The angles and the anchor**, driven through the production
//!    `EntityTable` tick and `rewo_world::cape::cape_angles`.
//! 3. **The four suppression gates**, driven through the production
//!    [`crate::live_cmd::resolve_cape`] against real jar equipment data, and
//!    through the real `route_set_entity_data` for the metadata bit.
//! 4. **The pixels**, by rendering a player wearing a **marker-coloured**
//!    cape offscreen and reading it back.
//!
//! # The detector
//!
//! The cape sheet the pixel witnesses upload is solid magenta, and an empty
//! frame is asserted to contain none of it. That is M38's discipline, adopted
//! here for the same reason: a default-Steve skin is full of browns and
//! blue-greys, and any "non-background" detector would count the player's own
//! body. A colour the rest of the frame cannot produce removes the class.
//!
//! # Every expectation is independent
//!
//! The rotation witnesses build their own `Rz·Ry·Rx` and their own 3×3 apply
//! rather than importing the renderer's, because the whole point of the
//! central witness is that those two orderings differ — reading the
//! comparison out of the code under test would assert nothing.

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::assets;
use rewo_data::equipment::ArmorLayer;
use rewo_gpu::entities::{
    cape_face_uv, cape_pool_geometry, cape_rotation, cape_transform, CapeDraw, EntityDraw,
    EntityModelKind,
};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_world::entities::EntityTable;

use crate::stats::OverlayRing;

/// 38 for the vanilla cape (M60), 26 for the wavy one (M61), 5 for M64 (the
/// re-projected collision + the inventory preview's cape).
const EXPECTED_WITNESSES: usize = 69;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 256;
const H: u32 = 256;

/// The pre-M60 atlas height. `CAPE_POOL_Y` must land exactly here: the band
/// was added below everything, so every address under it is unmoved.
const ATLAS_H_BEFORE_M60: u32 = 1408;

/// The rows M64 added to the **mob shelf region** for vanilla's 42 metadata
/// variant sheets. Unlike M22/M48/M60's bands this one is not at the bottom,
/// so it slid every dynamic pool — including the cape's — down with it.
const M64_SHELF_GROWTH: u32 = 128;

#[derive(ClapArgs)]
pub struct CapeshotArgs {
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
    /// Write the renders here, for eyeballing a failure.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[capeshot] {}  {name}: {detail}",
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

// ---- independent math, deliberately not the renderer's -------------------

type M3 = [[f32; 3]; 3];

fn ind_mul(a: M3, b: M3) -> M3 {
    let mut o = [[0f32; 3]; 3];
    for (r, row) in o.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

fn ind_apply(m: &M3, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| (0..3).map(|k| m[r][k] * v[k]).sum())
}

fn ind_x(a: f32) -> M3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn ind_y(a: f32) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn ind_z(a: f32) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// `ModelPart.translateAndRotate`'s `rotationZYX(z, y, x)` — the composition
/// **every other part** in the renderer uses, and the one the cape must not.
fn ind_zyx(x: f32, y: f32, z: f32) -> M3 {
    ind_mul(ind_mul(ind_z(z), ind_y(y)), ind_x(x))
}

const IDENT: ([[f32; 3]; 3], [f32; 3]) = (
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    [0.0; 3],
);

/// The cape's corners in model space, through the shipped transform **and**
/// the shipped clearance shift — the same two steps, in the same order,
/// `emit_cape` applies before the render flip.
fn cape_corners(cape: &CapeDraw, body: &([[f32; 3]; 3], [f32; 3])) -> Vec<[f32; 3]> {
    let (m, o) = cape_transform(body, cape);
    let s = rewo_gpu::entities::cape_clearance_shift(cape.chest_humanoid);
    let mut out = Vec::new();
    for (_f, pos, _uv) in rewo_gpu::mobs::cape_faces() {
        for corner in pos {
            let r = ind_apply(&m, corner);
            out.push([
                r[0] + o[0] + s[0],
                r[1] + o[1] + s[1],
                r[2] + o[2] + s[2],
            ]);
        }
    }
    out
}

const ZERO_CAPE: CapeDraw = CapeDraw {
    origin: (0, 0),
    flap: 0.0,
    lean: 0.0,
    lean2: 0.0,
    chest_humanoid: false,
    wavy: None,
};

// ---- 1. geometry + rotation ----------------------------------------------

fn check_geometry(c: &mut Checker) {
    let faces = rewo_gpu::mobs::cape_faces();
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let (mut umax, mut vmax) = (0f32, 0f32);
    for (_f, pos, uv) in &faces {
        for p in pos {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        for q in uv {
            umax = umax.max(q[0]);
            vmax = vmax.max(q[1]);
        }
    }
    c.record(
        "a1.the_cube_is_vanillas_10x16x1_slab",
        lo == [-5.0, 0.0, -1.0] && hi == [5.0, 16.0, 0.0] && faces.len() == 6,
        format!(
            "addBox(-5, 0, -1, 10, 16, 1) → {lo:?}..{hi:?}, 6 faces \
             (mutation: any other extent moves these)"
        ),
    );

    // The box-UV layout: u runs d+w+d+w = 1+10+1+10 = 22, v runs d+h = 17.
    c.record(
        "a2.box_uv_reaches_22x17_in_sheet_pixels",
        (umax - 22.0).abs() < 1e-6 && (vmax - 17.0).abs() < 1e-6,
        format!("u→{umax}, v→{vmax} (mutation: a mirrored or grown box shifts both)"),
    );

    // **The texHeight witness.** Normalize the emitted atlas V back into the
    // cape's own sheet: `v_sheet = (v_atlas * ATLAS_H - CAPE_POOL_Y) / slot_h`.
    // The box V of 17 must land 17/32, the `yTexScale 0.5` the cube carries.
    // Baking the slot 64 tall would put it at 17/64 — the mutation below.
    let (_aw, ah, _sw, sh, _slots, pool_y) = cape_pool_geometry();
    let uv = cape_face_uv((0, pool_y), &[[0.0, 17.0]; 4]);
    let v_sheet = (uv[0][1] * ah as f32 - pool_y as f32) / sh as f32;
    let want = 17.0 / 32.0;
    let if_64 = 17.0 / 64.0;
    c.record(
        "a3.the_sheet_is_64x32_so_v_normalizes_by_32_not_64",
        (v_sheet - want).abs() < 1e-4 && sh == 32,
        format!(
            "box v 17 → {v_sheet:.5} of the sheet (= 17/32 = {want:.5}); \
             MUTATION a 64-tall slot gives {if_64:.5}, halving every V"
        ),
    );
}

fn check_rotation(c: &mut Checker) {
    // At rest the cape hangs behind the torso. The body's back face is model
    // z = +2 (the box is z −2..+2), and the cape's nearest corner must not be
    // in front of it.
    let corners = cape_corners(&ZERO_CAPE, &IDENT);
    let zmin = corners.iter().fold(f32::MAX, |a, p| a.min(p[2]));
    let zmax = corners.iter().fold(f32::MIN, |a, p| a.max(p[2]));

    // MUTATION: apply the `PartPose`'s `Ry(π)` **as well**, which is what
    // happens if you read the pose as a rotation to compose rather than one
    // `setupAnim`'s leading `rotateY(-PI)` exists to cancel.
    let m_bad = ind_mul(
        ind_y(std::f32::consts::PI),
        cape_rotation(0.0, 0.0, 0.0),
    );
    let mut bad_zmin = f32::MAX;
    for (_f, pos, _uv) in rewo_gpu::mobs::cape_faces() {
        for corner in pos {
            bad_zmin = bad_zmin.min(ind_apply(&m_bad, corner)[2] + rewo_gpu::mobs::CAPE_PIVOT[2]);
        }
    }
    c.record(
        "b1.at_rest_the_cape_sits_behind_the_torsos_back_face",
        zmin >= 2.0 - 1e-4 && zmax > 2.0,
        format!(
            "cape z {zmin:.3}..{zmax:.3}, torso back face at z=2 — flush, never inside; \
             MUTATION applying the PartPose Ry(pi) too gives zmin {bad_zmin:.3}, \
             which buries it in the body"
        ),
    );
    c.record(
        "b2.the_double_rotation_mutation_is_actually_wrong",
        bad_zmin < 0.0,
        format!(
            "{bad_zmin:.3} < 0 — the mutation is not a near-miss, it rotates the \
             cape through the torso, so b1 is a real discriminator"
        ),
    );

    // The pose cancels exactly: at zero angles the net is Rx(6°)·Ry(180°).
    let got = cape_rotation(0.0, 0.0, 0.0);
    let want = ind_mul(ind_x(6f32.to_radians()), ind_y(std::f32::consts::PI));
    let dev = (0..3)
        .flat_map(|r| (0..3).map(move |k| (r, k)))
        .fold(0f32, |a, (r, k)| a.max((got[r][k] - want[r][k]).abs()));
    c.record(
        "b3.the_leading_rotateY_minus_pi_cancels_the_PartPose_exactly",
        dev < 1e-6,
        format!(
            "net == Rx(6deg)*Ry(180deg) to {dev:.2e} — the 6deg rest tilt survives, \
             the pose does not"
        ),
    );

    // **The central witness.** With a sideways sway the cape's own
    // `Rx·Rz·Ry` and the `Rz·Ry·Rx` every other part uses are different
    // rotations, and the difference is a visible fraction of a pixel on a
    // 16-px cape. This is the plausible-but-wrong drape.
    let sway = CapeDraw {
        lean2: 20.0,
        ..ZERO_CAPE
    };
    let a = (6.0f32 + sway.lean / 2.0 + sway.flap).to_radians();
    let b = (sway.lean2 / 2.0).to_radians();
    let cc = (180.0f32 - sway.lean2 / 2.0).to_radians();
    let wrong = ind_zyx(a, cc, b);
    let right = cape_rotation(sway.flap, sway.lean, sway.lean2);
    let mut worst = 0f32;
    for (_f, pos, _uv) in rewo_gpu::mobs::cape_faces() {
        for corner in pos {
            let p = ind_apply(&right, corner);
            let q = ind_apply(&wrong, corner);
            let d = (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f32>().sqrt();
            worst = worst.max(d);
        }
    }
    c.record(
        "b4.RxRzRy_differs_measurably_from_rotateZYX_of_the_same_angles",
        worst > 0.5,
        format!(
            "at capeLean2=20 the two orderings put a corner {worst:.3} px apart; \
             MUTATION composing them as mat_zyx(a, c, b) — what a Part would have \
             done — is this far wrong and would have looked plausible"
        ),
    );

    // And the two agree where they must: with only one non-zero angle there
    // is nothing to commute, so a witness that passed on *any* angles would
    // be measuring noise.
    let only_y = ind_zyx(0.0, cc, 0.0);
    let cape_only_y = {
        // flap and lean both feed `a`; setting them so a == 0 isolates Ry.
        let m = cape_rotation(-6.0, 0.0, 0.0);
        m
    };
    let dev2 = (0..3)
        .flat_map(|r| (0..3).map(move |k| (r, k)))
        .fold(0f32, |acc, (r, k)| {
            acc.max((cape_only_y[r][k] - ind_zyx(0.0, std::f32::consts::PI, 0.0)[r][k]).abs())
        });
    c.record(
        "b5.the_two_orderings_agree_when_only_one_angle_is_nonzero",
        dev2 < 1e-6 && only_y[0][0] != 0.0,
        format!(
            "with a=b=0 both are Ry(180deg) to {dev2:.2e} — so b4's gap is the \
             ordering, not a difference in the angles themselves"
        ),
    );
}

fn check_clearance(c: &mut Checker) {
    let plain = cape_corners(&ZERO_CAPE, &IDENT);
    let armored = cape_corners(
        &CapeDraw {
            chest_humanoid: true,
            ..ZERO_CAPE
        },
        &IDENT,
    );
    let dy = armored[0][1] - plain[0][1];
    let dz = armored[0][2] - plain[0][2];
    c.record(
        "b6.a_humanoid_chest_layer_shifts_the_cape_up_and_back",
        (dy - (-0.85)).abs() < 1e-4 && (dz - 1.1).abs() < 1e-4,
        format!(
            "delta ({dy:.4}, {dz:.4}) px = vanilla's (-0.053125, 0.06875) blocks x16; \
             model +y is world-down, so -0.85 lifts it clear"
        ),
    );
}

// ---- 2. the angles and the lagging anchor --------------------------------

fn check_anchor(c: &mut Checker) {
    // Driven through the production table: add, tick, read. The anchor is
    // private state; this is the only way to reach it, which is the point.
    let mut t = EntityTable::default();
    t.add(1, rewo_world::entities::EntityState::new(0, 0, 8.0, 0.0, 0.0, 0.0, 0.0));
    let mut ok = true;
    let mut seen = Vec::new();
    for n in 1..=6u32 {
        t.tick_lerp();
        let x = t.get(1).unwrap().cloak_pos(1.0)[0];
        // The entity never moves (no target set), so the gap closes as
        // `0.75^n` of the original 8 blocks — the exact geometric series.
        let want = 8.0 - 8.0 * 0.75f64.powi(n as i32);
        ok &= (x - want).abs() < 1e-9;
        seen.push(format!("{x:.4}"));
    }
    c.record(
        "c1.the_anchor_closes_a_quarter_of_the_gap_each_tick",
        ok,
        format!(
            "chasing 8.0: [{}] = 8*(1-0.75^n) exactly; MUTATION any other \
             coefficient diverges by tick 2",
            seen.join(", ")
        ),
    );

    // Over 10 blocks the axis teleports and rewrites `O`, so no partial tick
    // draws the cape mid-streak. Exactly 10 still eases — vanilla's test is
    // strict on both sides.
    let mut t2 = EntityTable::default();
    t2.add(1, rewo_world::entities::EntityState::new(0, 0, 11.0, 10.0, -11.0, 0.0, 0.0));
    t2.tick_lerp();
    let e = t2.get(1).unwrap();
    let at0 = e.cloak_pos(0.0);
    let at1 = e.cloak_pos(1.0);
    c.record(
        "c2.an_eleven_block_jump_snaps_and_rewrites_the_previous_slot",
        (at1[0] - 11.0).abs() < 1e-9
            && (at0[0] - 11.0).abs() < 1e-9
            && (at1[2] + 11.0).abs() < 1e-9
            && (at0[2] + 11.0).abs() < 1e-9
            && (at1[1] - 2.5).abs() < 1e-9,
        format!(
            "x {:.3} and z {:.3} snapped at BOTH partial ticks (O rewritten), \
             y at exactly 10 eased to {:.3}; MUTATION not rewriting O leaves \
             alpha=0 reading 0.0 and the cape streaks",
            at1[0], at1[2], at1[1]
        ),
    );
}

fn check_angles(c: &mut Checker) {
    use rewo_world::cape::{cape_angles, fall_flying_scale};

    // The clamp lands before the walk-bob add, so a *local* player walking
    // legitimately exceeds 32 degrees.
    let walk = std::f32::consts::FRAC_PI_2 / 6.0; // sin(walk*6) == 1
    let a = cape_angles([0.0, 10.0, 0.0], [0.0; 3], 0.0, 0.0, 0.5, walk);
    c.record(
        "c3.capeFlap_is_clamped_before_the_walk_bob_is_added",
        (a.flap - 48.0).abs() < 1e-3,
        format!(
            "deltaY 10 → raw 100, clamped 32, +sin*32*0.5 → {:.2}; MUTATION \
             clamping after would cap it at 32 and flatten every walk cycle",
            a.flap
        ),
    );

    // A remote player's walk term vanishes through `sin(walkDist * 6)`, not
    // through `bob` — which `RemotePlayer.tick` keeps updating.
    let r = cape_angles([0.0, 0.1, 0.0], [0.0; 3], 0.0, 0.0, 1.0, 0.0);
    c.record(
        "c4.a_remote_player_has_no_walk_term_even_at_full_bob",
        (r.flap - 1.0).abs() < 1e-4,
        format!(
            "bob=1.0, walkDist=0 → flap {:.4} = the deltaY term alone; \
             MUTATION gating the term on bob!=0 would look identical here and \
             be wrong, since only walkDist is structurally zero for a remote",
            r.flap
        ),
    );

    // Ten ticks of gliding squares to 1 and suppresses the lean entirely.
    let g = cape_angles([0.0, 0.0, -1.0], [0.0; 3], 0.0, 10.0, 0.0, 0.0);
    let g5 = cape_angles([0.0, 0.0, -1.0], [0.0; 3], 0.0, 5.0, 0.0, 0.0);
    c.record(
        "c5.ten_elytra_ticks_suppress_capeLean_completely",
        g.lean == 0.0 && fall_flying_scale(10.0) == 1.0 && (g5.lean - 75.0).abs() < 1e-3,
        format!(
            "scale(10)=1 → lean 0; at 5 ticks scale is 0.25 so lean is {:.1} of 100 \
             — squared, not linear",
            g5.lean
        ),
    );

    // The lower clamp is 0, so the cape never leans *forward*.
    let back = cape_angles([0.0, 0.0, -1.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    let front = cape_angles([0.0, 0.0, 1.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    let sat = cape_angles([0.0, 0.0, -2.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    c.record(
        "c6.capeLean_clamps_to_zero_below_not_to_minus_150",
        (back.lean - 100.0).abs() < 1e-3 && front.lean == 0.0 && sat.lean == 150.0,
        format!(
            "behind {:.0}, in front {:.0}, saturated {:.0}; MUTATION a symmetric \
             clamp would let a backward-walking player's cape swing through them",
            back.lean, front.lean, sat.lean
        ),
    );

    // capeLean2 is the cross product, so it is the sideways component and it
    // saturates at ±20.
    let side = cape_angles([1.0, 0.0, 0.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    let side2 = cape_angles([-1.0, 0.0, 0.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    c.record(
        "c7.capeLean2_is_the_cross_term_and_clamps_symmetrically",
        side.lean2 == -20.0 && side2.lean2 == 20.0 && side.lean == 0.0,
        format!(
            "pure sideways gap → lean2 {:.0}/{:.0}, lean {:.0} — the two axes do \
             not leak into each other",
            side.lean2, side2.lean2, side.lean
        ),
    );

    // capeFlap's own clamp is asymmetric: −6 up, 32 down.
    let up = cape_angles([0.0, 5.0, 0.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    let down = cape_angles([0.0, -5.0, 0.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
    c.record(
        "c8.capeFlap_clamps_asymmetrically_at_minus_6_and_32",
        up.flap == 32.0 && down.flap == -6.0,
        format!("+5 blocks → {:.0}, -5 → {:.0}", up.flap, down.flap),
    );
}

// ---- 3. the four suppression gates ---------------------------------------

struct Fixture {
    items: rewo_data::items::Items,
    equipment: rewo_data::equipment::EquipmentAssets,
    elytra: i32,
    chestplate: i32,
    pumpkin: i32,
}

fn fixture(baked: &assets::BakedAssets, paths: &rewo_data::DataPaths) -> Result<Fixture, String> {
    let items = rewo_data::items::Items::load(&paths.registries_json())?;
    let id = |n: &str| items.id(n).ok_or_else(|| format!("no item {n}"));
    Ok(Fixture {
        elytra: id("minecraft:elytra")?,
        chestplate: id("minecraft:iron_chestplate")?,
        pumpkin: id("minecraft:carved_pumpkin")?,
        equipment: baked.equipment.clone(),
        items,
    })
}

fn worn(item: i32) -> rewo_world::entities::WornPiece {
    rewo_world::entities::WornPiece {
        item,
        dye: None,
        trim: None,
        foil: false,
    }
}

/// A table holding one player, visible, cape shown, chest slot as given.
fn player_table(chest: Option<i32>) -> EntityTable {
    let mut t = EntityTable::default();
    t.add(1, rewo_world::entities::EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
    t.set_model_customisation(1, 0x01);
    if let Some(item) = chest {
        t.set_armor(1, 1, Some(worn(item)));
    }
    t
}

fn check_suppression(c: &mut Checker, f: &Fixture) {
    // The first cape slot, wherever the pool currently starts. Hard-coding it
    // would have made this fixture silently wrong the moment M64 moved the
    // pool, which is exactly what happened to `f2` below.
    let origin = Some(rewo_gpu::entities::cape_slot_origin(0));
    let resolve = |t: &EntityTable, kind| {
        crate::live_cmd::resolve_cape(t, 1, kind, 1.0, origin, &f.items, &f.equipment)
    };

    // The layer data these three rows turn on, asserted directly first — a
    // suppression witness that passed because the jar read nothing would be
    // indistinguishable from one that passed correctly.
    c.record(
        "d1.the_jar_gives_the_elytra_wings_and_the_chestplate_humanoid",
        f.equipment
            .has_layer("minecraft:elytra", ArmorLayer::Wings)
            && !f
                .equipment
                .has_layer("minecraft:elytra", ArmorLayer::Humanoid)
            && f.equipment
                .has_layer("minecraft:iron_chestplate", ArmorLayer::Humanoid)
            && !f
                .equipment
                .has_layer("minecraft:iron_chestplate", ArmorLayer::Wings),
        "elytra.json has only `wings`, iron.json has no `wings` — read from the real jar",
    );
    c.record(
        "d2.a_carved_pumpkin_names_no_equipment_asset_at_all",
        rewo_data::item_props_table::equip_asset("minecraft:carved_pumpkin").is_none()
            && !f
                .equipment
                .has_layer("minecraft:carved_pumpkin", ArmorLayer::Humanoid)
            && !f
                .equipment
                .has_layer("minecraft:carved_pumpkin", ArmorLayer::Wings),
        "Equippable.builder(HEAD) with no setAsset → both questions answer no",
    );

    c.record(
        "d3.an_equipped_elytra_suppresses_the_cape_entirely",
        resolve(&player_table(Some(f.elytra)), EntityModelKind::Player).is_none(),
        "hasLayer(WINGS) → no cape at all",
    );
    let plated = resolve(&player_table(Some(f.chestplate)), EntityModelKind::Player);
    c.record(
        "d4.a_chestplate_keeps_the_cape_and_shifts_it",
        plated.is_some_and(|d| d.chest_humanoid),
        "hasLayer(HUMANOID) → cape drawn, clearance shift on",
    );
    // **The row that separates the two questions.** An occupied chest slot is
    // not the rule; the *layer* is.
    let pumpkin = resolve(&player_table(Some(f.pumpkin)), EntityModelKind::Player);
    c.record(
        "d5.a_carved_pumpkin_in_the_chest_slot_keeps_the_cape_and_does_not_shift_it",
        pumpkin.is_some_and(|d| !d.chest_humanoid),
        "the only row where 'has a humanoid layer' and 'chest slot occupied' disagree; \
         MUTATION keying the shift on `chest.is_some()` passes d3 and d4 and fails here",
    );
    c.record(
        "d6.an_empty_chest_slot_keeps_the_cape_unshifted",
        resolve(&player_table(None), EntityModelKind::Player).is_some_and(|d| !d.chest_humanoid),
        "the control for d5 — same outcome, different reason",
    );

    // Gate 1, both halves.
    let mut inv = player_table(None);
    inv.set_shared_flags(1, 1 << 5);
    c.record(
        "d7.an_invisible_player_wears_no_cape",
        resolve(&inv, EntityModelKind::Player).is_none(),
        "shared flag 5",
    );

    let mut masks = Vec::new();
    for m in [0x01u8, 0x00, 0xFE, 0xFF] {
        let mut t = player_table(None);
        t.set_model_customisation(1, m);
        masks.push((m, resolve(&t, EntityModelKind::Player).is_some()));
    }
    c.record(
        "d8.only_bit_0_of_the_customisation_mask_shows_the_cape",
        masks == vec![(0x01, true), (0x00, false), (0xFE, false), (0xFF, true)],
        format!(
            "{masks:?} — PlayerModelPart.CAPE(0) so mask 1; 0xFE is every OTHER \
             part enabled and still hides it"
        ),
    );

    // Gate 2.
    c.record(
        "d9.no_uploaded_cape_means_no_cape",
        crate::live_cmd::resolve_cape(
            &player_table(None),
            1,
            EntityModelKind::Player,
            1.0,
            None,
            &f.items,
            &f.equipment,
        )
        .is_none(),
        "skin.cape() == null",
    );

    // The renderer gate: only `AvatarRenderer` adds a `CapeLayer`.
    c.record(
        "d10.a_zombie_with_every_other_condition_met_still_wears_no_cape",
        resolve(&player_table(None), EntityModelKind::Zombie).is_none()
            && resolve(&player_table(None), EntityModelKind::PlayerSlim).is_some(),
        "wears_cape is the renderer's rule, not the mesh's — the slim player is the control",
    );
}

// ---- 4. the wire ---------------------------------------------------------

fn check_wire(c: &mut Checker, paths: &rewo_data::DataPaths) -> Result<(), String> {
    // `{"textures":{"CAPE":{"url":"http://x/c"}}}` in base64 — written out
    // rather than encoded here so the fixture is the *wire* form the decoder
    // actually receives, and so this gate needs no base64 dependency.
    let b64 = "eyJ0ZXh0dXJlcyI6eyJDQVBFIjp7InVybCI6Imh0dHA6Ly94L2MifX19";
    let info = rewo_net::skins::decode_textures_property(b64);
    c.record(
        "e1.a_cape_only_profile_resolves_a_cape",
        info.as_ref()
            .is_some_and(|i| i.cape.as_deref() == Some("http://x/c") && i.url.is_none()),
        format!(
            "{:?}; MUTATION requiring a SKIN entry — which is what the decoder did \
             before M60 — returns None and loses the cape entirely",
            info.as_ref().map(|i| (i.url.is_some(), i.cape.is_some()))
        ),
    );

    // The index-16 BYTE, through the real packet-id selection seam.
    let packets = rewo_data::packets::Packets::load(&paths.packets_json())?;
    let ids = rewo_net::ids::Ids::resolve(&packets)?;
    let etypes = rewo_data::entity_types::EntityTypes::load(&paths.registries_json())?;
    let player = etypes.player_id;
    let zombie = etypes
        .id_of("minecraft:zombie")
        .ok_or("no minecraft:zombie")?;

    // eid varint, then (index, serializer=0 BYTE, value), then 0xFF.
    let body = |eid: u8, index: u8, value: u8| vec![eid, index, 0, value, 0xFF];
    let route = |type_id: i32, index: u8, kinds_player: Option<i32>| {
        let mut t = EntityTable::default();
        t.add(
            1,
            rewo_world::entities::EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0),
        );
        rewo_net::route_set_entity_data(
            ids.cb_play_set_entity_data,
            &body(1, index, 0x01),
            &ids,
            &mut t,
            rewo_net::MetaKinds {
                player: kinds_player,
                bee: None,
                guardian: None,
                elder_guardian: None,
                ..Default::default()
            },
        );
        t.shows_cape(1)
    };

    c.record(
        "e2.index_16_byte_is_the_customisation_mask_only_on_a_player",
        route(player, 16, Some(player)) && !route(zombie, 16, Some(player)),
        "a zombie's index-16 BYTE is somebody else's flags and must not toggle a cape",
    );
    c.record(
        "e3.reading_index_17_instead_would_show_nothing",
        !route(player, 17, Some(player)),
        "MUTATION the 1.21 index. `Avatar` was inserted between LivingEntity and \
         Player in 26.2 and defines MAIN_HAND then MODE_CUSTOMISATION, so they are \
         15 and 16 — at 17 the mask never arrives and every cape stays hidden",
    );
    c.record(
        "e4.an_unresolved_player_id_leaves_the_mask_alone",
        !route(player, 16, None),
        "fail-closed: no kind, no routing",
    );
    Ok(())
}

// ---- 5. the pool ---------------------------------------------------------

fn check_pool(c: &mut Checker) {
    let (aw, ah, sw, sh, slots, pool_y) = cape_pool_geometry();
    c.record(
        "f1.the_cape_band_is_flush_to_the_bottom_of_the_atlas",
        pool_y + (slots / (aw / sw)) * sh == ah && aw / sw == 16 && slots == 32,
        format!("{slots} slots of {sw}x{sh} at y={pool_y}, atlas {aw}x{ah}"),
    );
    c.record(
        "f2.the_cape_band_sits_exactly_where_the_shelf_growth_left_it",
        pool_y == ATLAS_H_BEFORE_M60 + M64_SHELF_GROWTH
            && ah == ATLAS_H_BEFORE_M60 + M64_SHELF_GROWTH + sh * 2,
        format!(
            "cape pool starts at {pool_y}, atlas {ah} tall. M60 put the band at \
             the pre-M60 ATLAS_H {ATLAS_H_BEFORE_M60} and claimed nothing below it \
             moved; M64 then grew the *shelf* region by {M64_SHELF_GROWTH} rows for \
             vanilla's 42 metadata variant sheets, and the shelf ceiling is defined \
             by subtraction from ATLAS_H — so the item, skin, trim and cape pools \
             all slid down by exactly that. Nothing on disk depends on those \
             origins, and the *mob* packing is still byte-for-byte what it was \
             because the packer is sequential and the region only grew at its far \
             end: `mobshot --check` staying at 243/243 is the empirical half of \
             that claim"
        ),
    );
}

// ---- 6. the wavy cape, on the CPU (M61) ----------------------------------
//
// `REWO_WAVY_CAPE_SPEC.md` is the source of truth for everything in this
// section: there is no vanilla behaviour to predict from, so every
// expectation is that file's table read back, and the strongest witness
// available is the reduction — which grades the new code against the *old,
// already-gated* code rather than against a restatement of itself.

use rewo_world::wavy_cape::{
    self, WavyCape, ANCHOR_ACCEL, DAMPING, GRAVITY, MAX_JOINT_RADIUS, RELAX_PASSES, REST_LEN,
    SEGMENTS, TORSO_RADIUS,
};

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn worst_link(c: &WavyCape) -> f64 {
    (0..c.segments())
        .map(|i| (dist(c.joints()[i], c.joints()[i + 1]) - REST_LEN).abs())
        .fold(0.0, f64::max)
}

/// A table with one caped player and the wavy cape switched on, driven
/// entirely through the production API.
fn wavy_table() -> EntityTable {
    let mut t = player_table(None);
    t.set_wavy_capes(true);
    t
}

fn check_wavy_constants(c: &mut Checker) {
    c.record(
        "w1.every_simulation_constant_is_the_specs_table",
        SEGMENTS == 16
            && GRAVITY == 0.008
            && DAMPING == 0.92
            && RELAX_PASSES == 4
            && REST_LEN == 1.0
            && TORSO_RADIUS == 2.5
            && MAX_JOINT_RADIUS == 24.0
            // The value, against a literal written out independently of the
            // expression that produces it.
            && (ANCHOR_ACCEL - 0.013962634015954637).abs() < 1e-17,
        format!(
            "SEGMENTS {SEGMENTS}, GRAVITY {GRAVITY}, DAMPING {DAMPING}, \
             RELAX_PASSES {RELAX_PASSES}, REST_LEN {REST_LEN}, TORSO_RADIUS \
             {TORSO_RADIUS}, MAX_JOINT_RADIUS {MAX_JOINT_RADIUS}, ANCHOR_ACCEL \
             {ANCHOR_ACCEL:.10} — the spec's values; MUTATION changing one in \
             code only fails here"
        ),
    );
    // REST_LEN is exactly 1.0 rather than 4/3, which is what SEGMENTS = 12
    // would have given — the bit-determinism witness wants an exactly
    // representable link length.
    c.record(
        "w2.the_renderers_joint_capacity_matches_the_simulations_length",
        rewo_gpu::entities::CAPE_MAX_JOINTS == SEGMENTS + 1 && REST_LEN.to_bits() == 1.0f64.to_bits(),
        format!(
            "CAPE_MAX_JOINTS {} == SEGMENTS+1 {}, and REST_LEN is exactly 1.0 \
             (16/16, a representable binary fraction); the renderer keeps its own \
             constant because rewo-gpu depends on no other rewo crate, so a \
             SEGMENTS bump that outgrew the array would silently truncate the \
             chain without this",
            rewo_gpu::entities::CAPE_MAX_JOINTS,
            SEGMENTS + 1
        ),
    );
}

/// The simulation derives the cape's attachment point itself (rewo-gpu is a
/// leaf crate and cannot be called from rewo-world). This is that
/// duplication's guard: the two derivations must agree, over angles that
/// exercise all three.
fn check_wavy_anchor(c: &mut Checker) {
    let mut worst = 0f64;
    for &(flap, lean, lean2) in &[
        (0.0f32, 0.0f32, 0.0f32),
        (32.0, 0.0, 0.0),
        (-6.0, 150.0, 0.0),
        (0.0, 0.0, 20.0),
        (12.0, 70.0, -20.0),
        (48.0, 33.0, 7.5),
    ] {
        let cape = CapeDraw {
            flap,
            lean,
            lean2,
            ..ZERO_CAPE
        };
        let (m, o) = cape_transform(&IDENT, &cape);
        // The spine's top corner: the cube spans z −1..0, so its centre line
        // is at local z −0.5.
        let r = ind_apply(&m, [0.0, 0.0, -0.5]);
        let want = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
        let got = wavy_cape::anchor_model(flap, lean, lean2);
        for k in 0..3 {
            worst = worst.max((got[k] - want[k] as f64).abs());
        }
    }
    c.record(
        "w3.the_simulations_anchor_is_the_renderers_attachment_point",
        worst < 1e-5,
        format!(
            "worst axis disagreement {worst:.2e} model units over six angle \
             triples, against the shipped `cape_transform`; MUTATION any drift \
             in the f64 copy of `cape_rotation` shows here"
        ),
    );

    // And the yaw: at 90° the attachment point has swung a quarter turn about
    // the body axis, so the radius is preserved and the axes have swapped.
    let a0 = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);
    let a90 = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 90.0);
    let r0 = (a0[0] * a0[0] + a0[2] * a0[2]).sqrt();
    let r90 = (a90[0] * a90[0] + a90[2] * a90[2]).sqrt();
    // At yaw 0 the cape hangs at world −Z (the model faces +Z), so the whole
    // radius is on Z; a quarter turn moves all of it onto +X.
    c.record(
        "w4.a_turn_swings_the_anchor_around_the_body_axis",
        (r0 - r90).abs() < 1e-9
            && (r0 - 2.4973).abs() < 1e-3
            && (a0[2] + r0).abs() < 1e-6
            && a0[0].abs() < 1e-6
            && (a90[0] - r0).abs() < 1e-6
            && a90[2].abs() < 1e-6
            && (a0[1] - a90[1]).abs() < 1e-12,
        format!(
            "radius {r0:.4} preserved and moved axis for axis — ({:.4}, {:.4}) \
             at yaw 0 becomes ({:.4}, {:.4}) at yaw 90, at unchanged height. \
             The anchor moving is what makes a *pure* turn wave the cape at \
             all, and it is why the simulation cannot live in a body-attached \
             frame: there, a turn would rotate the entire chain rigidly and \
             produce nothing",
            a0[0], a0[2], a90[0], a90[2]
        ),
    );
    // 2.4973 < TORSO_RADIUS: the push-out cylinder grazes the rest pose.
    c.record(
        "w5.the_torso_cylinder_grazes_the_rest_pose",
        r0 < TORSO_RADIUS && r0 > TORSO_RADIUS - 0.01,
        format!(
            "the vanilla spine leaves its pivot at radius {r0:.5}, {:.5} inside \
             TORSO_RADIUS {TORSO_RADIUS} — so 2.5 is not an arbitrary number, and \
             a rest chain is pushed out by three thousandths of a pixel and no more",
            TORSO_RADIUS - r0
        ),
    );
}

fn check_wavy_geometry(c: &mut Checker) {
    // The slab generator, at one slab, is the vanilla cube. This is a
    // structural check and NOT the reduction rule — that one is about
    // bypassing the simulation and is graded in pixels (see `y1`).
    let quads = rewo_gpu::mobs::cape_slab_quads(1);
    let faces = rewo_gpu::mobs::cape_faces();
    let mut ok = quads.len() == 6;
    if ok {
        for (q, (_f, pos, uv)) in quads.iter().zip(faces.iter()) {
            for k in 0..4 {
                // joint 0 is the cube's y = 0 edge, joint 1 its y = 16 edge.
                let want_y = if q.joint[k] == 0 { 0.0 } else { 16.0 };
                ok &= pos[k][1] == want_y;
                ok &= q.off[k][0] == pos[k][0];
                ok &= q.off[k][1] == pos[k][2] + 0.5;
                ok &= q.uv[k] == uv[k];
            }
        }
    }
    c.record(
        "w6.one_slab_is_the_vanilla_cube_face_for_face",
        ok,
        format!(
            "{} quads, every corner's offset and UV identical to \
             `cape_faces()`; MUTATION a UV shift applied to the caps, or an \
             off-by-one in the spine offset, breaks this before any pixel is drawn",
            quads.len()
        ),
    );

    // N slabs: the caps appear exactly once each, the sides tile the sheet's
    // full v 1..17 without gap or overlap, and every interior joint is shared
    // by the slabs above and below it — which is what makes the surface
    // watertight with no internal caps.
    let n = SEGMENTS;
    let q = rewo_gpu::mobs::cape_slab_quads(n);
    let caps = q.iter().filter(|q| q.joint[0] == q.joint[1] && q.joint[1] == q.joint[2]).count();
    let sides = q.len() - caps;
    let mut vmin = f32::MAX;
    let mut vmax = f32::MIN;
    let mut used = vec![0usize; n + 1];
    for quad in &q {
        for k in 0..4 {
            used[quad.joint[k]] += 1;
            if !(quad.joint[0] == quad.joint[1] && quad.joint[1] == quad.joint[2]) {
                vmin = vmin.min(quad.uv[k][1]);
                vmax = vmax.max(quad.uv[k][1]);
            }
        }
    }
    let interior_shared = (1..n).all(|j| used[j] >= 8);
    c.record(
        "w7.n_slabs_subdivide_the_same_sheet_and_share_their_boundaries",
        caps == 2
            && sides == 4 * n
            && (vmin - 1.0).abs() < 1e-4
            && (vmax - 17.0).abs() < 1e-4
            && interior_shared,
        format!(
            "{caps} caps + {sides} side faces for {n} slabs, side v spans \
             {vmin}..{vmax} = the single cube's own 1..17, and every interior \
             joint carries corners from both neighbours; MUTATION per-slab \
             frames or per-slab caps would show as a slit or as coincident \
             z-fighting quads at every joint"
        ),
    );
}

fn check_wavy_dynamics(c: &mut Checker) {
    let anchor = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);

    // Settling + constraints, on a chain driven by the production `tick`.
    let mut s = WavyCape::new(SEGMENTS, anchor);
    for _ in 0..600 {
        s.tick(anchor, [0.0; 3]);
    }
    let before = s.joints().to_vec();
    s.tick(anchor, [0.0; 3]);
    let drift = (0..before.len())
        .map(|j| dist(before[j], s.joints()[j]))
        .fold(0.0, f64::max);
    c.record(
        "w8.zero_motion_settles_to_an_idempotent_state",
        drift < 1e-6,
        format!(
            "after 600 still ticks one more moves every joint by {drift:.2e} \
             (< 1e-6); MUTATION DAMPING = 1.0 never sheds the energy gravity \
             puts in and this oscillates forever"
        ),
    );

    // The constraint residual, measured **after a whole tick** — which is
    // where M64 moved it. Before M64 this had to be read between the relax
    // passes and the push-out, because the collision response was the last
    // thing to run and was never re-projected; the post-tick number was a
    // different and much larger one. `solve` now interleaves the two
    // projections, so the finished state satisfies both.
    let rest_err = worst_link(&s);
    let mut m = WavyCape::new(SEGMENTS, anchor);
    let mut relaxed_err = 0f64;
    for t in 0..200 {
        let ph = t as f64 * 0.31;
        // A cloak gap swinging through a full circle at 1.5 blocks — the
        // largest vanilla's own `capeLean` clamp can represent.
        let f = [ph.sin() * 1.5, 0.0, ph.cos() * 1.5];
        // The three stages `tick` runs, in `tick`'s order, opened up so the
        // residual can be read at the named point. Not a reimplementation:
        // these are the same three methods.
        m.integrate(anchor, f);
        m.solve();
        m.clamp(anchor);
        relaxed_err = relaxed_err.max(m.worst_link_error());
    }
    // The push-out is what perturbs the links inside the solve, and a *turn*
    // is what fires it — the anchor swings to the far side of the body and
    // the chain has to cross the torso. Measured here rather than under the
    // forcing above, which blows the cape away from the body rather than
    // across it, so it never collides at all.
    let mut turning = WavyCape::new(SEGMENTS, wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0));
    let mut post_tick_err = 0f64;
    for step in 0..120 {
        let a = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, (step as f32 * 30.0).min(180.0));
        turning.tick(a, [0.0; 3]);
        post_tick_err = post_tick_err.max(turning.worst_link_error());
    }
    c.record(
        "w9.every_link_is_within_1e_4_of_REST_LEN_at_the_end_of_the_tick",
        rest_err < 1e-4 && relaxed_err < 1e-4 && post_tick_err < 1e-4,
        format!(
            "worst link error {rest_err:.2e} settled, {relaxed_err:.2e} under a \
             1.5-block gap swinging through a full circle, {post_tick_err:.2e} \
             through a 30-degree-per-tick turn — the one motion that fires the \
             push-out at all. That last number is M64's: the M61 order relaxed, \
             *then* collided once, and never re-projected, so the same turn \
             ended its tick 0.230 out — a fifth of a slab. `solve` now \
             interleaves the two projections, which converges geometrically \
             (2.30e-1 / 9.60e-3 / 4.27e-4 / 5.14e-5 at 1/2/3/4 passes), so the \
             spec's RELAX_PASSES = 4 is the first count that clears its own \
             1e-4. MUTATION RELAX_PASSES = 0 leaves the chain stretched by \
             whatever the integrator moved it; reverting to the M61 order \
             fails this row alone at 2.3e-1. NOTE the mass weighting is what \
             makes 1e-4 reachable at all: symmetric Gauss-Seidel measures \
             2.9e-2 here"
        ),
    );

    // Determinism, to the bit.
    let run = |seed: u64| -> Vec<[f64; 3]> {
        let mut w = WavyCape::new(SEGMENTS, anchor);
        let mut r = seed;
        for t in 0..300 {
            r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let f = |sh: u32| ((r >> sh) & 0xFFFF) as f64 / 65535.0 - 0.5;
            let a = wavy_cape::anchor_in_cape_space(
                (t % 39) as f32 - 6.0,
                (t % 151) as f32,
                (t % 41) as f32 - 20.0,
                (t * 7) as f32,
            );
            w.tick(a, [f(0) * 0.6, f(16) * 0.2, f(32) * 0.6]);
        }
        w.joints().to_vec()
    };
    let a = run(0x5EED);
    let b = run(0x5EED);
    c.record(
        "w10.two_runs_with_identical_inputs_are_bit_identical",
        a.iter()
            .zip(&b)
            .all(|(p, q)| (0..3).all(|k| p[k].to_bits() == q[k].to_bits())),
        "300 scripted ticks, compared as raw bits — no RNG, no wall clock, no \
         frame rate anywhere in the tick; MUTATION seeding anything from the \
         clock fails here immediately",
    );

    // Pinning, through the production table tick.
    let mut t = wavy_table();
    let mut pin_worst = 0f64;
    let mut seen = 0;
    for step in 0..120 {
        // Walk the player in a circle so the anchor moves every tick and the
        // cloak gap never closes.
        let ang = step as f64 * 0.11;
        let e = t.get_mut(1).unwrap();
        e.set_target(ang.cos() * 6.0, 0.0, ang.sin() * 6.0);
        e.set_rot((step * 13) as f32, 0.0);
        t.tick_lerp();
        let e = t.get(1).unwrap();
        let cloak = e.cloak_pos(1.0);
        let pos = e.render_pos(1.0);
        let a = rewo_world::cape::cape_angles(cloak, pos, e.yaw, e.fall_fly_ticks() as f32 + 1.0, 0.0, 0.0);
        let want = wavy_cape::anchor_in_cape_space(a.flap, a.lean, a.lean2, e.yaw);
        let sim = t.wavy_cape(1).expect("a caped player simulates");
        pin_worst = pin_worst.max(dist(sim.joints()[0], want));
        seen += 1;
    }
    c.record(
        "w11.joint_zero_is_the_vanilla_attachment_point_every_tick",
        pin_worst == 0.0 && seen == 120,
        format!(
            "{seen} ticks of a player walking a circle while turning, worst \
             deviation {pin_worst:e} — the pin is an assignment, not a very \
             stiff spring; MUTATION letting joint 0 simulate makes it lag by \
             whatever the anchor just moved"
        ),
    );
}

/// **The property that was wrong, and that nothing caught.**
///
/// The first M61 build fed the anchor gap in as an acceleration with no
/// coefficient. Every witness in this file passed — settling, constraints,
/// determinism, pinning, push-out, stability, the backstop, the reduction —
/// while the cape flew to 80.9° from vertical on a drift the vanilla cape
/// renders as 5°, because a delta in *blocks* is a hundred times gravity.
/// The gate could not see it because nothing here measured what the
/// simulation settles *to*.
///
/// So: settle the chain under a constant horizontal gap and read the angle.
/// The anchor sits far from the body axis on purpose — the torso push-out
/// would otherwise nudge the top joint and contaminate a measurement that is
/// about the integrator and the constraints, not the collision.
fn check_wavy_equilibrium(c: &mut Checker) {
    let anchor = [0.0, 0.0, -100.0];
    let settled_tilt = |d: f64| -> f64 {
        let mut w = WavyCape::new(SEGMENTS, anchor);
        for _ in 0..4000 {
            w.tick(anchor, [d, 0.0, 0.0]);
        }
        let tip = w.joints()[SEGMENTS];
        let v = [
            tip[0] - anchor[0],
            tip[1] - anchor[1],
            tip[2] - anchor[2],
        ];
        v[0].hypot(v[2]).atan2(-v[1]).to_degrees()
    };
    // Vanilla's own answer for the same gap, through the shipped
    // `cape_angles`: a cloak `d` blocks behind at yaw 0 leans `100·d`.
    let vanilla_lean =
        |d: f64| rewo_world::cape::cape_angles([0.0, 0.0, -d], [0.0; 3], 0.0, 0.0, 0.0, 0.0).lean as f64;

    // The physics, first: the tilt is `atan(a_h / g)` across the whole range
    // a real cloak gap can reach (vanilla clamps `capeLean` at 150, i.e. 1.5
    // blocks), not only where it happens to look linear.
    let mut worst = 0f64;
    let mut rows = Vec::new();
    for &d in &[0.0125f64, 0.05, 0.2, 0.4, 0.864, 1.5] {
        let got = settled_tilt(d);
        let want = (ANCHOR_ACCEL * d / GRAVITY).atan().to_degrees();
        worst = worst.max((got - want).abs());
        rows.push(format!("{d} -> {got:.2} deg (vanilla {:.1})", vanilla_lean(d)));
    }
    c.record(
        "w20.the_settled_tilt_is_atan_of_the_anchor_acceleration_over_gravity",
        worst < 1e-3,
        format!(
            "worst deviation {worst:.2e} degrees from atan(ANCHOR_ACCEL*delta/\
             GRAVITY) over gaps [{}]. MUTATION dropping the coefficient — \
             feeding the delta in raw, which is what the first M61 build did \
             and what nothing here caught — reads 80.9 degrees at a 0.05 gap \
             instead of 4.99",
            rows.join(", ")
        ),
    );

    // And the constant's own derivation: at small angles `atan(x) ≈ x`, so
    // the cloth's tilt must be vanilla's `100·delta` degrees. This is where
    // ANCHOR_ACCEL comes from, so it is the row that pins it.
    let small: Vec<(f64, f64, f64)> = [0.0125f64, 0.025, 0.05]
        .iter()
        .map(|&d| (d, settled_tilt(d), vanilla_lean(d)))
        .collect();
    let worst_small = small
        .iter()
        .fold(0f64, |a, (_, got, van)| a.max((got - van).abs()));
    c.record(
        "w21.at_small_angles_the_cloth_settles_where_vanillas_capeLean_points",
        worst_small < 0.05,
        format!(
            "{} — worst disagreement {worst_small:.3} degrees. This is the \
             derivation: `ANCHOR_ACCEL = GRAVITY * 100 * pi/180` is exactly \
             what makes `theta ≈ a_h/g` equal vanilla's 100 degrees per block \
             in the small-angle limit, so it is derived and not tuned",
            small
                .iter()
                .map(|(d, got, van)| format!("gap {d}: cloth {got:.2}, vanilla {van:.2}"))
                .collect::<Vec<_>>()
                .join("; ")
        ),
    );

    // The divergence at larger gaps is *intended* and is asserted rather than
    // tolerated, so nobody later "fixes" it with a second factor.
    let (walk, sprint) = (settled_tilt(0.4), settled_tilt(0.864));
    c.record(
        "w22.and_it_compresses_where_vanillas_linear_clamp_does_not",
        walk < vanilla_lean(0.4) - 3.0 && sprint < vanilla_lean(0.864) - 20.0 && walk > 30.0,
        format!(
            "a walk-sized gap of 0.4 settles at {walk:.1} degrees against \
             vanilla's {:.1}, a sprint's 0.864 at {sprint:.1} against {:.1}. \
             INTENDED: `atan` is the angle a hanging cloth actually takes and \
             vanilla's `100*delta` is its linear approximation, so they must \
             part company away from zero. Asserted, not tolerated — a second \
             coefficient chasing vanilla's number out here would fail this",
            vanilla_lean(0.4),
            vanilla_lean(0.864)
        ),
    );
}

fn check_wavy_pushout(c: &mut Checker) {
    // A scripted 180° turn: the anchor swings to the far side of the body and
    // the chain has to cross the torso to follow it.
    let mut c0 = WavyCape::new(SEGMENTS, wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0));
    for _ in 0..200 {
        c0.tick(wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0), [0.0; 3]);
    }
    let mut min_r = f64::MAX;
    let mut grazed = false;
    for step in 0..80 {
        let yaw = (step as f32 * 30.0).min(180.0);
        c0.tick(wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, yaw), [0.0; 3]);
        for p in &c0.joints()[1..] {
            let r = (p[0] * p[0] + p[2] * p[2]).sqrt();
            min_r = min_r.min(r);
            // A joint sitting on the cylinder to within float noise is the
            // push-out's own fingerprint: a free chain lands there with
            // probability zero.
            grazed |= (r - TORSO_RADIUS).abs() < 1e-9;
        }
    }
    c.record(
        "w12.a_180_degree_turn_leaves_no_joint_inside_the_torso_cylinder",
        min_r >= TORSO_RADIUS - 1e-9,
        format!(
            "closest approach {min_r:.6} vs TORSO_RADIUS {TORSO_RADIUS}; the \
             prototype measured 0.458 with the push-out disabled, i.e. the cape \
             swinging clean through the player's chest. NOTE the rule builds a \
             *cylinder*, and the torso AABB is 8 wide, so a joint at x 3.5, z 0 \
             is outside the cylinder and inside the box — this asserts what the \
             rule actually creates"
        ),
    );
    c.record(
        "w13.the_push_out_demonstrably_engages_during_that_turn",
        grazed,
        "at least one joint sits exactly on the cylinder — the push-out's own \
         signature, and what stops w12 passing vacuously because nothing ever \
         came near the body",
    );

    // M64: the same turn, run twice — once through the production `tick`, and
    // once through an explicitly reconstructed M61 stage order (all the relax
    // passes, then one push-out). Both chains see identical anchors, so the
    // only difference between them is where the collision sits in the solve.
    //
    // This is the one witness that can tell the two orders apart. w9 asserts
    // the residual is small and w12 asserts the cylinder holds; neither says
    // *which* order produced them, and the M61 build passed both — it read
    // its residual at a point the finished state no longer occupied.
    let seed = |c: &mut WavyCape| {
        for _ in 0..200 {
            c.tick(wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0), [0.0; 3]);
        }
    };
    let mut shipped = WavyCape::new(SEGMENTS, wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0));
    let mut m61 = WavyCape::new(SEGMENTS, wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0));
    seed(&mut shipped);
    seed(&mut m61);
    let (mut shipped_err, mut m61_err, mut m61_min_r) = (0f64, 0f64, f64::MAX);
    for step in 0..120 {
        let a = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, (step as f32 * 30.0).min(180.0));
        shipped.tick(a, [0.0; 3]);
        shipped_err = shipped_err.max(shipped.worst_link_error());
        // The M61 order, from the same public stages: relax, relax, relax,
        // relax, collide once.
        m61.integrate(a, [0.0; 3]);
        for _ in 0..wavy_cape::RELAX_PASSES {
            m61.relax_pass();
        }
        m61.push_out();
        m61.clamp(a);
        m61_err = m61_err.max(m61.worst_link_error());
        for p in &m61.joints()[1..] {
            m61_min_r = m61_min_r.min((p[0] * p[0] + p[2] * p[2]).sqrt());
        }
    }
    c.record(
        "w23.interleaving_the_collision_into_the_solve_re_projects_it",
        shipped_err < 1e-4 && m61_err > 0.1 && m61_min_r >= TORSO_RADIUS - 1e-9,
        format!(
            "through the same 30-degree-per-tick turn the shipped solve ends \
             each tick {shipped_err:.2e} out of REST_LEN and the reconstructed \
             M61 order {m61_err:.3} — a fifth of a slab, left there because a \
             joint the push-out shoved off the torso was never re-projected. \
             Both orders end on the push-out, which is why the M61 chain still \
             reaches {m61_min_r:.6}: the fix is where the collision sits, not \
             whether it runs. MUTATION reverting `solve` to the M61 order makes \
             the two numbers equal and fails this row (and w9) alone"
        ),
    );
}

fn check_wavy_stability(c: &mut Checker) {
    // 600 adversarial ticks: teleports, the >10-block cloak snap, and a fixed
    // pseudo-random shove, all through the production table so the cloak
    // anchor's own snap branch is what produces the >10-block jumps.
    let mut t = wavy_table();
    let mut r = 0xC0FFEEu64;
    let mut worst_reach = 0f64;
    let mut nan = false;
    for step in 0..600 {
        r = r.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let f = |sh: u32| ((r >> sh) & 0xFFFF) as f64 / 65535.0 - 0.5;
        let e = t.get_mut(1).unwrap();
        if step % 97 == 0 {
            // Well past `moveCloak`'s 10-block threshold, so the anchor
            // teleports and rewrites its own previous slot.
            e.set_target(f(0) * 4000.0, f(16) * 4000.0, f(32) * 4000.0);
        } else {
            e.nudge(f(0) * 0.6, f(16) * 0.2, f(32) * 0.6);
        }
        e.set_rot(f(48) as f32 * 720.0, 0.0);
        t.tick_lerp();
        let sim = t.wavy_cape(1).unwrap();
        let anchor = sim.joints()[0];
        for p in sim.joints() {
            nan |= p.iter().any(|v| !v.is_finite());
            worst_reach = worst_reach.max(dist(*p, anchor));
        }
    }
    let reach = REST_LEN * SEGMENTS as f64;
    c.record(
        "w14.600_adversarial_ticks_stay_finite_and_within_the_chains_own_reach",
        !nan && worst_reach <= reach + 1e-6,
        format!(
            "teleports every 97th tick past the 10-block cloak snap, pseudo-random \
             shoves between, yaw sweeping 720 degrees: worst joint {worst_reach:.6} \
             from its anchor against the chain's own {reach} of link length, no \
             NaN; MUTATION dropping the snap handling feeds the chain a \
             thousand-block anchor step every tick"
        ),
    );
}

fn check_wavy_backstop(c: &mut Checker) {
    let anchor = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);
    let mut s = WavyCape::new(SEGMENTS, anchor);
    for _ in 0..100 {
        s.tick(anchor, [0.0; 3]);
    }
    // **The divergence has to be constructed.** `MAX_JOINT_RADIUS` is
    // unreachable while the constraints hold — the chain is 16 units of link
    // and the relax satisfies every link exactly — so "no joint beyond 24"
    // passes whether or not the clamp exists. What the constraints cannot
    // absorb is a link whose squared length overflows: `relax` declines it
    // by design rather than inventing a direction, and the clamp is what
    // catches the result.
    s.tick(anchor, [1e200, 0.0, 0.0]);
    let finite = s.joints().iter().all(|p| p.iter().all(|v| v.is_finite()));
    let worst = s
        .joints()
        .iter()
        .map(|p| dist(*p, anchor))
        .fold(0.0, f64::max);
    let clamped = s
        .joints()
        .iter()
        .any(|p| (dist(*p, anchor) - MAX_JOINT_RADIUS).abs() < 1e-9);
    c.record(
        "w15.the_backstop_engages_on_a_divergence_the_constraints_decline",
        finite && worst <= MAX_JOINT_RADIUS + 1e-9 && clamped,
        format!(
            "a 1e200 impulse leaves the chain finite with its worst joint at \
             {worst:.6} — exactly MAX_JOINT_RADIUS, so the clamp *fired* rather \
             than the constraints having quietly absorbed it; MUTATION removing \
             the clamp leaves that joint at 1e200 and every later tick keeps it \
             there"
        ),
    );
    s.tick(anchor, [0.0; 3]);
    c.record(
        "w16.and_the_chain_recovers_to_within_REST_LEN_tolerance",
        worst_link(&s) < 1e-4,
        format!(
            "one tick later the worst link is {:.2e} off REST_LEN — the clamp \
             hands the solver a finite state it can fix, which is the whole \
             point of clamping rather than tolerating",
            worst_link(&s)
        ),
    );
}

fn check_wavy_interpolation(c: &mut Checker) {
    let anchor = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);
    let mut s = WavyCape::new(SEGMENTS, anchor);
    for t in 0..30 {
        s.tick(anchor, [(t as f64 * 0.2).sin() * 0.5, 0.0, 0.0]);
    }
    let snapshot = s.clone();
    let mut out = [[0.0f32; 3]; rewo_gpu::entities::CAPE_MAX_JOINTS];
    for a in [0.0, 0.25, 0.5, 0.75, 1.0] {
        s.interpolated(a, &mut out);
    }
    let unchanged = s == snapshot;
    s.interpolated(0.0, &mut out);
    let at0 = out[SEGMENTS][0] == s.prev_joints()[SEGMENTS][0] as f32;
    s.interpolated(1.0, &mut out);
    let at1 = out[SEGMENTS][0] == s.joints()[SEGMENTS][0] as f32;
    s.interpolated(0.5, &mut out);
    let mid = (s.prev_joints()[SEGMENTS][0] + s.joints()[SEGMENTS][0]) * 0.5;
    let at_half = (out[SEGMENTS][0] - mid as f32).abs() < 1e-6;
    // And the two ends must actually differ, or "interpolates" is vacuous.
    let moving = (s.prev_joints()[SEGMENTS][0] - s.joints()[SEGMENTS][0]).abs() > 1e-3;
    c.record(
        "w17.a_frame_interpolates_and_never_advances_the_simulation",
        unchanged && at0 && at1 && at_half && moving,
        format!(
            "five interpolations leave the state bit-identical; alpha 0 is the \
             previous tick, 1 is this one, 0.5 the midpoint, and the two ends are \
             {:.4} apart so the test is not reading a static chain",
            (s.prev_joints()[SEGMENTS][0] - s.joints()[SEGMENTS][0]).abs()
        ),
    );
}

fn check_wavy_lifecycle(c: &mut Checker) {
    let mut t = wavy_table();
    t.tick_lerp();
    let on = t.wavy_cape(1).is_some();
    // The metadata bit going out takes the chain with it.
    t.set_model_customisation(1, 0x00);
    t.tick_lerp();
    let bit_off = t.wavy_cape(1).is_none();
    t.set_model_customisation(1, 0x01);
    t.tick_lerp();
    let back = t.wavy_cape(1).is_some();
    t.remove(1);
    let removed = t.wavy_cape(1).is_none();

    // And the flag itself: default-off means no chain is ever built, which is
    // what keeps M60's 38 witnesses out of reach of this feature.
    let mut plain = player_table(None);
    plain.tick_lerp();
    let default_off = !plain.wavy_capes_enabled() && plain.wavy_cape(1).is_none();
    let mut off_again = wavy_table();
    off_again.tick_lerp();
    off_again.set_wavy_capes(false);
    let cleared = off_again.wavy_cape(1).is_none();

    // **The reduction rule's mutation, proven numerically.** The spec's first
    // draft said the reduction would hold "with infinite stiffness"; it would
    // not. Stiffness fixes a link's *length*, not its orientation, so a rigid
    // two-joint chain is a pendulum and hangs straight down — while the
    // vanilla cape sits at `Rx(6 + capeLean/2 + capeFlap)`. Here is that gap,
    // measured, which is what makes the bypass load-bearing rather than a
    // coincidence the code could be allowed to lose.
    let anchor = wavy_cape::anchor_in_cape_space(0.0, 0.0, 0.0, 0.0);
    let mut one = WavyCape::new(1, anchor);
    for _ in 0..600 {
        one.tick(anchor, [0.0; 3]);
    }
    let hem = one.joints()[1];
    let swing = [
        hem[0] - anchor[0],
        hem[1] - anchor[1],
        hem[2] - anchor[2],
    ];
    // The vanilla spine direction in the same space: the cape rotation's
    // second column (model +y, down the cape), through the model→cape map at
    // yaw 0.
    let m = cape_rotation(0.0, 0.0, 0.0);
    let theta = 180f32.to_radians();
    let (st, ct) = theta.sin_cos();
    let spine = rewo_gpu::entities::model_dir_to_cape([m[0][1], m[1][1], m[2][1]], st, ct, 0.0, 1.0);
    let ln = (swing[0].powi(2) + swing[1].powi(2) + swing[2].powi(2)).sqrt();
    let dot = (0..3)
        .map(|k| swing[k] / ln * spine[k] as f64)
        .sum::<f64>()
        .clamp(-1.0, 1.0);
    let angle = dot.acos().to_degrees();
    let hem_gap = 16.0 * (angle.to_radians() / 2.0).sin() * 2.0;
    // 6° of vanilla rest tilt, less the 0.157° the push-out adds by nudging
    // the free end from the anchor's radius 2.4973 out to TORSO_RADIUS —
    // `asin(0.00274 / REST_LEN)`, in the same direction, so it subtracts.
    let nudge = ((TORSO_RADIUS - 2.49726) / REST_LEN).asin().to_degrees();
    c.record(
        "w18.a_simulated_single_segment_is_a_pendulum_not_the_vanilla_cape",
        (angle - (6.0 - nudge)).abs() < 0.02 && hem_gap > 1.5,
        format!(
            "a settled one-segment chain hangs {angle:.3} degrees off the \
             vanilla spine — the 6-degree rest tilt of `Rx(6 + capeLean/2 + \
             capeFlap)` less the {nudge:.3} degrees the push-out lifts the free \
             end by — putting the hem {hem_gap:.3} model units away. No amount \
             of stiffness could have produced that angle: stiffness fixes a \
             link's *length*, not its orientation. So `y1`'s bit-identity is \
             the *bypass*, and MUTATION letting the single segment simulate is \
             a real, visible difference"
        ),
    );

    c.record(
        "w19.a_chain_exists_exactly_while_the_flag_and_the_cape_bit_do",
        on && bit_off && back && removed && default_off && cleared,
        "on with the bit, gone without it, gone on entity removal, never built \
         with the feature off, and dropped the moment it is switched off — so a \
         recycled entity id cannot inherit a chain and the vanilla path cannot \
         be reached through a stale one",
    );
}

// ---- 7. the pixels -------------------------------------------------------

/// Count marker-magenta pixels. Nothing else in the frame can be magenta: the
/// clear is black, the world is empty, and the player wears the jar's default
/// skin. See the module header — this is M38's rule.
fn magenta(img: &[u8]) -> u32 {
    let mut n = 0;
    for px in img.chunks_exact(4) {
        if px[0] > 150 && px[2] > 150 && px[1] < 90 {
            n += 1;
        }
    }
    n
}

/// The marker pixels inside `(x, y, w, h)` of a `W`x`H` frame (M64).
fn magenta_in(img: &[u8], (x, y, w, h): (u32, u32, u32, u32)) -> u32 {
    let mut n = 0;
    for row in y..(y + h).min(H) {
        for col in x..(x + w).min(W) {
            let i = (row * W + col) as usize * 4;
            let px = &img[i..i + 4];
            if px[0] > 150 && px[2] > 150 && px[1] < 90 {
                n += 1;
            }
        }
    }
    n
}

/// The marker pixels as a mask, for comparing two silhouettes (M61).
fn magenta_mask(img: &[u8]) -> Vec<bool> {
    img.chunks_exact(4)
        .map(|px| px[0] > 150 && px[2] > 150 && px[1] < 90)
        .collect()
}

/// Intersection over union of two silhouettes, 0..1.
///
/// This compares two **offscreen renders of the same scene that differ only
/// in the cape mode** — same camera, same skin, same angles, same draw list
/// otherwise — with a colour the rest of the frame cannot produce. It is not
/// the frame-diff §0.0 forbids: M50's control differed in 41,284 pixels
/// because its two frames came from two *live* runs whose worlds had drifted,
/// and M37's because the trigger mutated the world it was measured against.
/// Nothing here is live and nothing mutates.
fn silhouette_iou(a: &[u8], b: &[u8]) -> f64 {
    let (ma, mb) = (magenta_mask(a), magenta_mask(b));
    let mut inter = 0u32;
    let mut union = 0u32;
    for (x, y) in ma.iter().zip(&mb) {
        inter += u32::from(*x && *y);
        union += u32::from(*x || *y);
    }
    if union == 0 {
        return 0.0;
    }
    inter as f64 / union as f64
}

fn check_pixels(
    c: &mut Checker,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    f: &Fixture,
    args: &CapeshotArgs,
) -> Result<(), String> {
    let mut off = Offscreen::new(gpu, W, H)?;
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
    let mut wr = WorldRenderer::new(gpu, off.format, 16, &layers)?;

    let r = (|| -> Result<(), String> {
        wr.init_entities(
            gpu,
            crate::live_cmd::font_data(baked),
            crate::live_cmd::entity_textures(baked),
        )?;
        // The same rule M45 wrote down: a gate that calls `init_entities`
        // directly must install whatever the live path installs on top of it.
        if let Some(g) = baked.glint.as_ref() {
            wr.init_entity_glint(gpu, &g.rgba, g.w, g.h)?;
        }
        if let Some(g) = baked.armor_glint.as_ref() {
            wr.init_entity_armor_glint(gpu, &g.rgba, g.w, g.h)?;
        }
        wr.set_held_items(crate::live_cmd::to_gpu_held_items(&baked.held_items));

        // A solid-magenta 64x32 cape.
        let sheet: Vec<u8> = (0..CAPE_TEXELS).flat_map(|_| [255u8, 0, 255, 255]).collect();
        let origin = wr
            .upload_player_cape(gpu, &sheet)
            .ok_or("cape upload failed")?;

        // Behind the player: at yaw 0 a model faces +Z, and the cape hangs at
        // −Z of the body in world space, so the camera looks along +Z.
        let eye = Vec3::new(0.0, 1.0, -2.6);
        let dir = Vec3::new(0.0, 0.0, 1.0);
        wr.set_camera(eye.to_array());
        let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
        let proj = Mat4::from_cols_array_2d(&perspective_reverse_z(
            60f32.to_radians(),
            W as f32 / H as f32,
            0.05,
        ));
        let vp = (proj * view).to_cols_array_2d();

        // The billboard basis the nametag pass wants; no nametags are drawn
        // here, but `set_entities` takes it either way.
        let cam_right = [1.0f32, 0.0, 0.0];
        let cam_up = [0.0f32, 1.0, 0.0];
        let mut shot = |gpu: &mut Gpu,
                        off: &mut Offscreen,
                        wr: &mut WorldRenderer,
                        draws: &[EntityDraw<'_>]|
         -> Result<Vec<u8>, String> {
            wr.set_entities(draws, cam_right, cam_up, 0.0);
            off.render(gpu, Some((&mut *wr, vp)), &draw, CLEAR)?;
            off.read_rgba(gpu)
        };

        let cape = CapeDraw {
            origin,
            flap: 0.0,
            lean: 0.0,
            lean2: 0.0,
            chest_humanoid: false,
            wavy: None,
        };
        let empty = shot(gpu, &mut off, &mut wr, &[])?;
        let bare = shot(gpu, &mut off, &mut wr, &[player_draw(None)])?;
        let caped = shot(gpu, &mut off, &mut wr, &[player_draw(Some(cape))])?;
        let again = shot(gpu, &mut off, &mut wr, &[])?;

        if let Some(dir) = &args.out_dir {
            std::fs::create_dir_all(dir).map_err(|e| format!("out-dir: {e}"))?;
            off.save_png(gpu, &dir.join("cape.png")).ok();
        }

        c.record(
            "g1.an_empty_frame_contains_no_marker_colour",
            magenta(&empty) == 0,
            "so every count below is the cape and nothing else",
        );
        c.record(
            "g2.a_bare_player_contains_no_marker_colour_either",
            magenta(&bare) == 0,
            format!(
                "the default skin produces 0 magenta px — the detector cannot \
                 match the body it is drawn over ({} px in the caped frame)",
                magenta(&caped)
            ),
        );
        let n = magenta(&caped);
        c.record(
            "g3.the_cape_renders_behind_the_player_and_is_visible",
            n > 500,
            format!("{n} marker px from behind, over a bare frame's 0"),
        );
        c.record(
            "g4.clearing_the_draw_list_restores_the_empty_frame_exactly",
            again == empty,
            "byte-identical, so no witness above read a stale buffer",
        );
        // Suppression, all the way to pixels.
        let mut t = player_table(Some(f.elytra));
        t.set_model_customisation(1, 0x01);
        let suppressed = crate::live_cmd::resolve_cape(
            &t,
            1,
            EntityModelKind::Player,
            1.0,
            Some(origin),
            &f.items,
            &f.equipment,
        );
        let elytra_frame = shot(gpu, &mut off, &mut wr, &[player_draw(suppressed)])?;
        c.record(
            "g5.an_elytra_wearer_draws_zero_cape_pixels",
            magenta(&elytra_frame) == 0 && elytra_frame == bare,
            "and the frame is byte-identical to the bare player — the suppression \
             removes the geometry rather than hiding it",
        );

        // ---- M61: the reduction, and the wave --------------------------
        //
        // Both cape modes are resolved through the production
        // `resolve_cape`, so the chain reaching the renderer is the one the
        // real table simulated and interpolated — not a fixture posing as
        // one (M45's and M41's gates both quietly stopped testing their
        // subject by reimplementing a slice of the app).
        let resolve_wavy = |t: &EntityTable| {
            crate::live_cmd::resolve_cape(
                t,
                1,
                EntityModelKind::Player,
                1.0,
                Some(origin),
                &f.items,
                &f.equipment,
            )
            .expect("a caped player resolves")
        };

        // A still player, settled.
        let mut rest_t = wavy_table();
        for _ in 0..600 {
            rest_t.tick_lerp();
        }
        let rest_w = resolve_wavy(&rest_t);
        let rest_v = CapeDraw {
            wavy: None,
            ..rest_w
        };

        // **The reduction.** One segment, taken from the same simulation,
        // must render the vanilla cape *bit for bit* — because the emitter
        // bypasses the simulation entirely at that length rather than
        // trusting it to converge.
        let sim1 = {
            let anchor = wavy_cape::anchor_in_cape_space(rest_w.flap, rest_w.lean, rest_w.lean2, 0.0);
            let mut s = WavyCape::new(1, anchor);
            for _ in 0..600 {
                s.tick(anchor, [0.0; 3]);
            }
            let mut buf = [[0.0f32; 3]; rewo_gpu::entities::CAPE_MAX_JOINTS];
            let n = s.interpolated(1.0, &mut buf);
            rewo_gpu::entities::CapeJoints::from_slice(&buf[..n]).unwrap()
        };
        let one_seg = CapeDraw {
            wavy: Some(sim1),
            ..rest_v
        };
        let f_rest_v = shot(gpu, &mut off, &mut wr, &[player_draw(Some(rest_v))])?;
        let f_one = shot(gpu, &mut off, &mut wr, &[player_draw(Some(one_seg))])?;
        c.record(
            "y1.at_one_segment_the_wavy_cape_is_the_vanilla_cape_bit_for_bit",
            f_one == f_rest_v && magenta(&f_rest_v) > 500,
            format!(
                "{} marker px, and the two frames compare equal as raw bytes. \
                 The chain handed in is a *settled pendulum* hanging {} — see \
                 w18 — so this is the bypass and not a convergence coincidence; \
                 MUTATION letting the single segment simulate moves the hem 1.7 \
                 model units and the frames stop matching",
                magenta(&f_rest_v),
                "straight down"
            ),
        );

        // A player sprinting sideways: the cloak lags, which is the only
        // thing that lifts the vanilla cape and the only forcing the
        // simulation gets besides gravity.
        let mut mot_t = wavy_table();
        for _ in 0..60 {
            mot_t.get_mut(1).unwrap().nudge(0.30, 0.0, 0.0);
            mot_t.tick_lerp();
        }
        let mot_w = resolve_wavy(&mot_t);
        let mot_v = CapeDraw {
            wavy: None,
            ..mot_w
        };
        let f_rest_w = shot(gpu, &mut off, &mut wr, &[player_draw(Some(rest_w))])?;
        let f_mot_v = shot(gpu, &mut off, &mut wr, &[player_draw(Some(mot_v))])?;
        let f_mot_w = shot(gpu, &mut off, &mut wr, &[player_draw(Some(mot_w))])?;
        if let Some(dir) = &args.out_dir {
            std::fs::write(dir.join("wavy_rest.rgba"), &f_rest_w).ok();
            std::fs::write(dir.join("wavy_motion.rgba"), &f_mot_w).ok();
        }
        let iou_rest = silhouette_iou(&f_rest_v, &f_rest_w);
        let iou_mot = silhouette_iou(&f_mot_v, &f_mot_w);
        c.record(
            "y2.at_rest_the_wave_barely_moves_the_silhouette",
            iou_rest > 0.90,
            format!(
                "IoU {iou_rest:.4} against the vanilla cape from directly \
                 behind. NOT 1.000, and the residue is exactly identifiable: \
                 gravity is world-down, so a settled chain hangs vertically \
                 while vanilla's rest pose is `Rx(6°)` — 6 degrees of tilt \
                 that is nearly along this camera's view axis and moves the \
                 hem 1.7 units in depth. Reading 'downward' as the *cape's* \
                 local down would make this exactly 1.0 and would cost the \
                 reduction its mutation partner; see the milestone report"
            ),
        );
        c.record(
            "y3.under_motion_the_two_capes_are_grossly_different",
            iou_mot < 0.65 && iou_mot < iou_rest - 0.25 && magenta(&f_mot_w) > 500,
            format!(
                "IoU {iou_mot:.4} sprinting sideways, against {iou_rest:.4} at \
                 rest — the cloth swings where the rigid slab tilts by \
                 capeLean2/2. {} marker px, so this is a moved cape and not a \
                 vanished one",
                magenta(&f_mot_w)
            ),
        );
        c.record(
            "y4.the_wave_is_reproducible_frame_to_frame",
            shot(gpu, &mut off, &mut wr, &[player_draw(Some(mot_w))])? == f_mot_w,
            "re-rendering the same draw gives a byte-identical frame — the \
             emitter reads the chain and never advances it, so a paused game \
             cannot drift",
        );

        // ---- M64: the inventory preview's cape -------------------------
        //
        // The preview is a **second** `EntityPass` with its own atlas (M36:
        // two `set_draws` into one vertex ring would cross the draws), and
        // it draws only when the container screen is open — so this needs
        // the screen too. Nothing is live and nothing mutates: the two
        // frames below differ only in whether the preview draw carries a
        // cape.
        wr.set_entities(&[], cam_right, cam_up, 0.0);
        let preview_ok = crate::live_cmd::container_sprites(baked)
            .map(|s| wr.init_container(gpu, &s))
            .transpose()?
            .is_some()
            && wr
                .init_preview(gpu, crate::live_cmd::font_data(baked), crate::live_cmd::entity_textures(baked))
                .map(|_| wr.preview_ready())
                .unwrap_or(false);
        if preview_ok {
            wr.set_container(true, None);
            // The preview's **own** upload. The world pass already holds this
            // exact sheet at `origin`; the address is only meaningful in the
            // atlas it came from.
            let p_origin = wr
                .upload_preview_cape(gpu, &sheet)
                .ok_or("preview cape upload failed")?;
            // A second slot in the *world* pass, claimed before the render
            // closure borrows the renderer. It exists so p3 can move the two
            // pools' cursors apart — see there.
            let green: Vec<u8> = (0..CAPE_TEXELS).flat_map(|_| [0u8, 255, 0, 255]).collect();
            let world_second = wr
                .upload_player_cape(gpu, &green)
                .ok_or("second world cape upload failed")?;
            let (rx, ry, rw, rh) = rewo_gpu::container::preview_rect(W as f32, H as f32);
            let window = (rx as u32, ry as u32, rw as u32, rh as u32);
            let vp_prev = rewo_gpu::container::preview_view_proj(W as f32, H as f32, 1.8, 0.0);
            let rect = ash::vk::Rect2D {
                offset: ash::vk::Offset2D { x: rx as i32, y: ry as i32 },
                extent: ash::vk::Extent2D { width: rw as u32, height: rh as u32 },
            };
            let mut prev = |cape: Option<CapeDraw>, yaw: f32| -> Result<Vec<u8>, String> {
                let mut d = player_draw(cape);
                d.yaw = yaw;
                d.head_yaw = yaw;
                wr.set_preview(Some((&d, vp_prev, rect)));
                off.render(gpu, Some((&mut wr, vp)), &draw, CLEAR)?;
                off.read_rgba(gpu)
            };
            // Built by the **production** resolver, so a client that stopped
            // giving the preview a cape could not pass p1 with a draw this
            // gate assembled for itself (M45's and M41's failure mode).
            let marker = crate::live_cmd::preview_cape(Some(p_origin));
            // Two poses. `bodyRot = 180 + xAngle` plus the camera's own half
            // turn is what makes the shipped preview face you — and a cape
            // seen from the front is nearly all body, so it would be graded
            // by a handful of edge pixels. Turning the model the other way
            // shows the whole sheet and makes the count unambiguous; the
            // facing-you number is reported beside it because that is the
            // pose a player actually sees.
            let p_back = prev(marker, 0.0)?;
            let p_front = prev(marker, 180.0)?;
            let p_bare = prev(None, 0.0)?;
            let inside = magenta_in(&p_back, window);
            let outside = magenta(&p_back) - inside;
            let front = magenta_in(&p_front, window);
            // The cape's front face is 10x16 model units = 0.625 x 1.0 blocks,
            // and the preview's scale is `guiScale * 30` px per block — so a
            // fully visible one covers `0.625*s * 1.0*s` px. Half of that is
            // the floor: the arms and legs eat into it from a straight-on
            // view, and a threshold derived from the geometry beats a round
            // number nobody can check.
            let (_, _, gs) = rewo_gpu::container::gui_origin(W as f32, H as f32);
            let full = (0.625 * gs * 30.0) * (gs * 30.0);
            c.record(
                "p1.the_inventory_preview_wears_its_cape",
                inside as f32 > full * 0.5 && front > 0 && magenta(&p_bare) == 0,
                format!(
                    "{inside} marker px inside the preview's {}x{} window with \
                     the model turned away — against {full:.0} for a wholly \
                     unoccluded 10x16 sheet at this scale — {front} in the pose \
                     a player actually sees (the body hides all but the edges), \
                     and {} on a bare preview. The preview is a second \
                     EntityPass with its own atlas, so before M64 there was no \
                     cape pool in it to address and the draw carried \
                     `cape: None`",
                    window.2,
                    window.3,
                    magenta(&p_bare)
                ),
            );
            c.record(
                "p2.and_it_stays_inside_the_window_the_panel_paints",
                outside == 0,
                format!(
                    "{outside} marker px outside it — the preview is scissored \
                     to the black rectangle `inventory.png` paints, so a cape \
                     hanging off the model cannot spill across the slots"
                ),
            );
            // **Why the second upload exists.** The two passes hold separate
            // atlases, so an origin is only meaningful in the one it came
            // from — and because both pools fill from empty, the *first* cape
            // in each lands at the same texel, which would let a borrowed
            // address look correct forever. Claiming a second world slot
            // moves the two apart and makes the mistake observable: the
            // preview asked to draw from an address that is populated in the
            // world's atlas and empty in its own must render no cape.
            let borrowed = prev(
                marker.map(|m| CapeDraw {
                    origin: world_second,
                    ..m
                }),
                0.0,
            )?;
            c.record(
                "p3.a_cape_address_borrowed_from_the_world_pass_draws_nothing_here",
                world_second != p_origin && magenta_in(&borrowed, window) == 0 && inside > 0,
                format!(
                    "the world pass's second cape slot is {world_second:?} where \
                     the preview's first is {p_origin:?}; drawing the preview \
                     from the world's address yields {} marker px against {inside} \
                     from its own. Both pools start empty, so the *first* cape in \
                     each lands on the same texel — which is exactly why reusing \
                     an address would have looked right until a second player \
                     joined. p1 renders magenta only because the sheet was \
                     uploaded into this atlas too",
                    magenta_in(&borrowed, window)
                ),
            );
            let none = crate::live_cmd::preview_cape(None);
            c.record(
                "p4.the_preview_hangs_a_cape_exactly_when_it_has_a_slot_for_one",
                none.is_none()
                    && marker.is_some_and(|m| {
                        m.origin == p_origin
                            && m.flap == 0.0
                            && m.lean == 0.0
                            && m.lean2 == 0.0
                            && !m.chest_humanoid
                            && m.wavy.is_none()
                    }),
                format!(
                    "no slot -> {none:?}; a slot -> {marker:?}. The three angles \
                     are zero because they are driven entirely by the gap \
                     between the player and their lagging cloak anchor, and a \
                     player standing in an open inventory has let it close — \
                     the *moving* preview is missing for the same reason its \
                     legs are, not by a different simplification. \
                     `chest_humanoid` is false because the preview draws no \
                     armour at all, so neither of `CapeLayer`'s other two \
                     gates has anything to act on"
                ),
            );
            wr.set_preview(None);
            wr.set_container(false, None);
        } else {
            return Err("preview/container passes unavailable".into());
        }
        Ok(())
    })();
    wr.destroy(gpu);
    off.destroy(gpu);
    r
}

const CAPE_TEXELS: usize = 64 * 32;

fn player_draw<'a>(cape: Option<CapeDraw>) -> EntityDraw<'a> {
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
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        ground_age: None,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None; 2],
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        skin_uv: None,
        scale_mul: 1.0,
        armor: [None; 4],
        mount: None,
        light: [1.0, 1.0, 1.0],
        emissive: Default::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        anim_id: 0.0,
        cape,
    }
}

pub fn run(args: CapeshotArgs) -> Result<(), String> {
    println!("[capeshot] mode: check (the oracle asserts unconditionally)");
    let paths = rewo_data::DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let f = fixture(&baked, &paths)?;

    // Validation is on in every build, debug AND release.
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    let status = if gpu.validation_active {
        "ON"
    } else if args.no_validation {
        "off (--no-validation)"
    } else {
        "off (VK_LAYER_KHRONOS_validation unavailable)"
    };
    println!("[capeshot] Vulkan validation: {status}");
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "capeshot check: Vulkan validation requested but not active — install \
             the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass --no-validation"
                .into(),
        );
    }

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_geometry(&mut c);
    check_rotation(&mut c);
    check_clearance(&mut c);
    check_anchor(&mut c);
    check_angles(&mut c);
    check_suppression(&mut c, &f);
    check_wire(&mut c, &paths)?;
    check_pool(&mut c);
    check_wavy_constants(&mut c);
    check_wavy_anchor(&mut c);
    check_wavy_geometry(&mut c);
    check_wavy_dynamics(&mut c);
    check_wavy_equilibrium(&mut c);
    check_wavy_pushout(&mut c);
    check_wavy_stability(&mut c);
    check_wavy_backstop(&mut c);
    check_wavy_interpolation(&mut c);
    check_wavy_lifecycle(&mut c);
    check_pixels(&mut c, &mut gpu, &baked, &f, &args)?;

    println!(
        "[capeshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
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
    println!("[capeshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}
