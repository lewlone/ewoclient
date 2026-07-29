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

const EXPECTED_WITNESSES: usize = 38;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 256;
const H: u32 = 256;

/// The pre-M60 atlas height. `CAPE_POOL_Y` must land exactly here: the band
/// was added below everything, so every address under it is unmoved.
const ATLAS_H_BEFORE_M60: u32 = 1408;

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
    let origin = Some((0u32, 1408u32));
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
        "f2.the_atlas_grew_by_exactly_the_new_band_so_nothing_below_it_moved",
        pool_y == ATLAS_H_BEFORE_M60 && ah == ATLAS_H_BEFORE_M60 + sh * 2,
        format!(
            "cape pool starts at {pool_y} = the pre-M60 ATLAS_H, so every mob, item, \
             skin and trim texel address is unchanged — `mobshot --check` staying at \
             243/243 is the empirical half of this claim"
        ),
    );
}

// ---- 6. the pixels -------------------------------------------------------

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
