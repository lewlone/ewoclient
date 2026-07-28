//! `rewo handshot --check` — the first-person hand oracle (M38).
//!
//! Serverless, validation-required, fail-closed. Three layers:
//!
//! 1. **The bake**, on the real client jar — the two first-person display
//!    transforms and the rules that decide what a missing one means.
//! 2. **The placement**, driving the production geometry builder and comparing
//!    against a derivation done here from the decompile's constants.
//! 3. **The pixels**, rendering through the production pass and reading back.
//!
//! # The detector
//!
//! Every pixel witness below uses a **synthetic magenta texture** rather than a
//! real item's, and measures inside a bounded window. That is deliberate.
//! M38's own development produced three false measurements in a row, each the
//! same shape — a detector matching more than its subject: non-black pixels
//! against a painted sky, brown pixels against a brown hotbar, cyan against a
//! blue sky. A colour the rest of the frame cannot produce removes the whole
//! class, and it is the only reason these numbers can be trusted.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use glam::{Mat4, Vec4};
use rewo_data::{assets, DataPaths};
use rewo_gpu::hand::{
    build_vertices, display_transform, item_hand, player_arm, Arm, EquipHeight, HandDraw,
    SwingKind, ViewBob,
};
use rewo_gpu::held::{DisplayTransform, HeldItemModel, HeldQuad};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, SkyMode, WorldRenderer};
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

const EXPECTED_WITNESSES: usize = 29;
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 640;
const H: u32 = 360;
/// The vertical FOV `Camera.calculateHudFov` hard-codes for the hand — not the
/// player's FOV option, which the world uses and effects modify.
const HUD_FOV: f32 = 70.0;
const NEAR: f32 = 0.05;

#[derive(ClapArgs, Debug)]
pub struct HandshotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[handshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: HandshotArgs) -> Result<(), String> {
    println!("[handshot] the oracle asserts unconditionally (--check selects the exit code)");
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir for version data")?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_bake(&mut c, &baked);
    check_placement(&mut c);
    check_pixels(&mut c, &args)?;

    println!(
        "[handshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
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
    println!("[handshot] PASS — {} witnesses", c.witnessed);
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

// -- 1. the bake, on the real jar ---------------------------------------------

fn check_bake(c: &mut Checker, baked: &assets::BakedAssets) {
    let held = &baked.held_items;
    let sword = held.any("minecraft:diamond_sword");
    let dirt = held.any("minecraft:dirt");
    c.record(
        "b1.both_geometry_paths_carry_a_first_person_transform",
        sword.is_some_and(|m| m.first_right.scale[0] != 1.0)
            && dirt.is_some_and(|m| m.first_right.scale[0] != 1.0),
        format!(
            "sword scale {:?}, dirt scale {:?} — an item that reached the hand with an \
             identity transform would render at four times the size and in the wrong \
             place, so a non-unit scale is the cheapest proof the chain was walked",
            sword.map(|m| m.first_right.scale[0]),
            dirt.map(|m| m.first_right.scale[0]),
        ),
    );

    // `item/handheld`: rotation [0,-90,25], translation [1.13,3.2,1.13] (the
    // deserializer's x0.0625 already applied), scale 0.68.
    let ok = sword.is_some_and(|m| {
        m.first_right.rotation == [0.0, -90.0, 25.0]
            && (m.first_right.translation[0] - 1.13 * 0.0625).abs() < 1e-6
            && (m.first_right.scale[0] - 0.68).abs() < 1e-6
    });
    c.record(
        "b2.a_handheld_item_takes_handhelds_own_first_person_entry",
        ok,
        format!("{:?}", sword.map(|m| m.first_right)),
    );

    // `block/block`: rotation [0,45,0], scale 0.4 — and the left entry is
    // [0,225,0], *not* the mirror of the right.
    let ok = dirt.is_some_and(|m| {
        m.first_right.rotation == [0.0, 45.0, 0.0]
            && (m.first_right.scale[0] - 0.4).abs() < 1e-6
            && m.first_left.rotation == [0.0, 225.0, 0.0]
    });
    c.record(
        "b3.a_block_declares_both_hands_independently",
        ok,
        format!(
            "right {:?} left {:?} — 225 is 45 plus a half turn, not the negation the \
             draw-time mirror applies. The block models author the two separately, so \
             deriving one from the other is wrong even though it looks symmetric",
            dirt.map(|m| m.first_right.rotation),
            dirt.map(|m| m.first_left.rotation),
        ),
    );

    // `item/generated` declares no left entry, so every item whose chain stops
    // there inherits the right one. An **apple** is generated; a stick is not —
    // it parents `item/handheld`, which authors both hands. Picking the stick
    // here first made this witness fail against correct code, so both are
    // asserted: one separates the fallback from the authored case.
    let apple = held.any("minecraft:apple");
    let stick = held.any("minecraft:stick");
    c.record(
        "b4.a_generated_sprite_falls_back_to_the_right_hand_entry",
        apple.is_some_and(|m| m.first_left == m.first_right)
            && stick.is_some_and(|m| m.first_left != m.first_right),
        format!(
            "apple {:?} equals its right entry; stick {:?} does not, because \
             `item/handheld` declares its own. `ItemTransforms`' builder replaces an \
             absent first-person left with the right, and has no such line for the \
             third-person pair",
            apple.map(|m| m.first_left.rotation),
            stick.map(|m| m.first_left.rotation),
        ),
    );
}

// -- 2. the placement ----------------------------------------------------------

/// A 0..16 cube with one magenta texture, as the block bake produces.
fn cube_item(first_right: DisplayTransform, first_left: DisplayTransform) -> HeldItemModel {
    let mut quads = Vec::new();
    // Six faces, so the shape bounds in every direction.
    let f = |dir: u8, verts: [[f32; 3]; 4]| HeldQuad {
        verts,
        uv: [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        tex: 0,
        part: 0,
        dir,
    };
    let (a, b) = (0.0f32, 16.0f32);
    quads.push(f(5, [[b, a, a], [b, b, a], [b, b, b], [b, a, b]]));
    quads.push(f(4, [[a, a, a], [a, b, a], [a, b, b], [a, a, b]]));
    quads.push(f(1, [[a, b, a], [b, b, a], [b, b, b], [a, b, b]]));
    quads.push(f(0, [[a, a, a], [b, a, a], [b, a, b], [a, a, b]]));
    quads.push(f(3, [[a, a, b], [b, a, b], [b, b, b], [a, b, b]]));
    quads.push(f(2, [[a, a, a], [b, a, a], [b, b, a], [a, b, a]]));
    HeldItemModel {
        quads,
        right: DisplayTransform::default(),
        left: DisplayTransform::default(),
        ground: DisplayTransform::default(),
        gui: DisplayTransform::default(),
        first_right,
        first_left,
        from_block: true,
    }
}

fn block_transform() -> DisplayTransform {
    DisplayTransform {
        rotation: [0.0, 45.0, 0.0],
        translation: [0.0; 3],
        scale: [0.4, 0.4, 0.4],
    }
}

fn projection() -> Mat4 {
    Mat4::from_cols_array_2d(&perspective_reverse_z(
        HUD_FOV.to_radians(),
        W as f32 / H as f32,
        NEAR,
    ))
}

/// Project a built vertex list to the screen bounding box.
fn screen_bbox(verts: &[rewo_gpu::gui_item::GuiItemVertex]) -> (f32, f32, f32, f32) {
    let proj = projection();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for v in verts {
        let clip = proj * Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
        if clip.w <= 0.0 {
            continue;
        }
        let sx = (clip.x / clip.w + 1.0) / 2.0 * W as f32;
        // The pass draws through a flipped viewport: clip +y is up.
        let sy = (1.0 - clip.y / clip.w) / 2.0 * H as f32;
        x0 = x0.min(sx);
        x1 = x1.max(sx);
        y0 = y0.min(sy);
        y1 = y1.max(sy);
    }
    (x0, y0, x1, y1)
}

fn draw_one(arm: Arm, item: &HeldItemModel, attack: f32, inverse_height: f32) -> Vec<rewo_gpu::gui_item::GuiItemVertex> {
    build_vertices(
        Mat4::IDENTITY,
        &[HandDraw {
            arm,
            item: Some(item),
            attack,
            inverse_height,
            swings: SwingKind::Whack,
            main_hand: true,
            using: None,
            is_shield: false,
        }],
        &|_| Some([0.0, 0.0, 1.0, 1.0]),
        None,
    )
}

fn check_placement(c: &mut Checker) {
    let item = cube_item(block_transform(), block_transform());
    let rest = draw_one(Arm::Right, &item, 0.0, 0.0);
    let (x0, y0, x1, y1) = screen_bbox(&rest);

    // Derived here from the decompile: the pose is
    // `translate(0.56, -0.52, -0.72)`, the transform is `block/block`'s
    // `rotation [0,45,0] scale 0.4`, and the projection is the hud FOV.
    let want = {
        let m = Mat4::from_translation(glam::Vec3::new(0.56, -0.52, -0.72))
            * Mat4::from_rotation_y(45f32.to_radians())
            * Mat4::from_scale(glam::Vec3::splat(0.4))
            * Mat4::from_translation(glam::Vec3::splat(-0.5));
        let proj = projection();
        let (mut a, mut b, mut cc, mut d) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for i in 0..8 {
            let p = glam::Vec3::new(
                (i & 1) as f32,
                ((i >> 1) & 1) as f32,
                ((i >> 2) & 1) as f32,
            );
            let clip = proj * (m * p.extend(1.0));
            let sx = (clip.x / clip.w + 1.0) / 2.0 * W as f32;
            let sy = (1.0 - clip.y / clip.w) / 2.0 * H as f32;
            a = a.min(sx);
            cc = cc.max(sx);
            b = b.min(sy);
            d = d.max(sy);
        }
        (a, b, cc, d)
    };
    let close = |p: f32, q: f32| (p - q).abs() < 0.5;
    c.record(
        "p1.a_held_block_lands_where_the_decompile_puts_it",
        close(x0, want.0) && close(y0, want.1) && close(x1, want.2) && close(y1, want.3),
        format!(
            "built ({x0:.1}, {y0:.1})..({x1:.1}, {y1:.1}) against a derivation done \
             here of ({:.1}, {:.1})..({:.1}, {:.1})",
            want.0, want.1, want.2, want.3
        ),
    );

    // The left hand mirrors in x and nowhere else.
    let left = draw_one(Arm::Left, &item, 0.0, 0.0);
    let (lx0, ly0, lx1, ly1) = screen_bbox(&left);
    let centre = W as f32 / 2.0;
    c.record(
        "p2.the_left_hand_is_the_right_mirrored_about_the_screen_centre",
        ((x0 - centre) + (lx1 - centre)).abs() < 1.0
            && ((x1 - centre) + (lx0 - centre)).abs() < 1.0
            && close(ly0, y0)
            && close(ly1, y1),
        format!(
            "right x {x0:.1}..{x1:.1}, left x {lx0:.1}..{lx1:.1} about {centre:.0}; \
             y unchanged ({y0:.1}..{y1:.1} against {ly0:.1}..{ly1:.1}). `invert` \
             negates the arm's x offset and the mirror negates the transform's, and \
             neither touches y"
        ),
    );

    // A swing moves it; the same call with no swing does not.
    let swung = draw_one(Arm::Right, &item, 0.5, 0.0);
    let (sx0, sy0, ..) = screen_bbox(&swung);
    let still = build_vertices(
        Mat4::IDENTITY,
        &[HandDraw {
            arm: Arm::Right,
            item: Some(&item),
            attack: 0.5,
            inverse_height: 0.0,
            swings: SwingKind::None,
            main_hand: true,
            using: None,
            is_shield: false,
        }],
        &|_| Some([0.0, 0.0, 1.0, 1.0]),
        None,
    );
    c.record(
        "p3.a_swing_moves_the_item_and_a_none_animation_does_not",
        ((sx0 - x0).abs() > 5.0 || (sy0 - y0).abs() > 5.0) && still == rest,
        format!(
            "swinging moves the corner from ({x0:.1}, {y0:.1}) to ({sx0:.1}, {sy0:.1}); \
             `SwingAnimation.NONE` is byte-identical to rest, not a small nudge"
        ),
    );

    // The equip dip lowers it by exactly 0.6 blocks.
    let dipped = draw_one(Arm::Right, &item, 0.0, 1.0);
    let lowered = {
        let a = item_hand(Arm::Right, 0.0, 0.0, true).transform_point3(glam::Vec3::ZERO);
        let b = item_hand(Arm::Right, 1.0, 0.0, true).transform_point3(glam::Vec3::ZERO);
        b.y - a.y
    };
    let (_, dy0, ..) = screen_bbox(&dipped);
    c.record(
        "p4.the_equip_dip_lowers_the_item_by_six_tenths_of_a_block",
        (lowered - -0.6).abs() < 1e-6 && dy0 > y0,
        format!(
            "{lowered:.3} blocks, and the top edge moves down the screen from \
             {y0:.1} to {dy0:.1}. `ITEM_HEIGHT_SCALE` is negative — the item drops \
             out of frame during a swap, it does not rise out of it"
        ),
    );

    // The clock itself: three ticks down, the swap at the bottom, then back.
    let mut h = EquipHeight::default();
    for _ in 0..8 {
        h.tick(Some(1));
    }
    let raised = h.inverse(1.0);
    h.tick(Some(2));
    let after_one = h.inverse(1.0);
    let visible_after_one = h.visible_item();
    h.tick(Some(2));
    h.tick(Some(2));
    c.record(
        "p5.a_swap_dips_over_three_ticks_and_changes_the_item_out_of_frame",
        raised.abs() < 1e-6
            && (after_one - 0.4).abs() < 1e-6
            && visible_after_one == Some(1)
            && h.visible_item() == Some(2),
        format!(
            "raised {raised:.2}, one tick later {after_one:.2} and still showing the \
             old item, swapped by the third. The 0.4 clamp is what makes it three \
             ticks rather than a snap"
        ),
    );

    // The view sway is a tenth of the camera's lead over its lagged copy.
    let bob = ViewBob::default();
    let (sway_x, _) = bob.sway(40.0, 0.0, 1.0);
    let mut settled_bob = ViewBob::default();
    for _ in 0..40 {
        settled_bob.tick(40.0, 0.0);
    }
    let (settled, _) = settled_bob.sway(40.0, 0.0, 1.0);
    c.record(
        "p6.the_view_sway_is_a_tenth_of_the_lag_and_decays",
        (sway_x - 4.0).abs() < 1e-4 && settled.abs() < 1e-3,
        format!(
            "a 40 degree flick sways {sway_x:.2} degrees and decays to \
             {settled:.4} once the bob catches up"
        ),
    );

    // The bare arm's chain is not the item's with different numbers.
    let arm_rest = player_arm(Arm::Right, 0.0, 0.0).transform_point3(glam::Vec3::ZERO);
    c.record(
        "p7.the_bare_arm_chain_places_a_shoulder_not_a_hand",
        arm_rest.y < -0.9 && arm_rest.z < -0.5 && arm_rest.x > 0.0,
        format!(
            "the arm part's origin lands at ({:.2}, {:.2}, {:.2}) — below and in \
             front of the eye. Its pre- and post-rotation translates are in block \
             units, and with the cube vertices divided by 16 the whole chain is \
             self-consistent; without that divide the arm is ten blocks away",
            arm_rest.x, arm_rest.y, arm_rest.z
        ),
    );

    // -- the use-driven poses ----------------------------------------------
    use rewo_gpu::hand::{use_hand, UseAnim, UsePose};
    let pose = |anim: UseAnim, remaining: i32, duration: i32| UsePose {
        anim,
        remaining,
        duration,
    };
    // **Sample an offset point, not the origin.** A rotation applied last
    // leaves the origin exactly where it was, so `transform_point3(ZERO)` is
    // blind to it — which is how the first cut of `u3` measured the brush's
    // sweep as motionless.
    let probe = glam::Vec3::new(0.0, 0.5, 0.0);
    let at = |p: &UsePose, shield: bool| {
        use_hand(Arm::Right, 0.0, p, 1.0, shield).map(|m| m.transform_point3(probe))
    };

    // `hasCustomArmTransform` is true for exactly three, and it inverts the
    // order of two calls: the pose runs *before* the resting offset rather
    // than after. Composing the other way is a plausible tidying that moves an
    // eaten apple most of a block.
    let custom: Vec<UseAnim> = [
        UseAnim::None, UseAnim::Eat, UseAnim::Drink, UseAnim::Block, UseAnim::Bow,
        UseAnim::Trident, UseAnim::Crossbow, UseAnim::Spyglass, UseAnim::TootHorn,
        UseAnim::Brush, UseAnim::Bundle, UseAnim::Spear,
    ]
    .into_iter()
    .filter(|a| a.has_custom_arm_transform())
    .collect();
    c.record(
        "u1.exactly_three_use_animations_invert_the_transform_order",
        custom == vec![UseAnim::Eat, UseAnim::Drink, UseAnim::Spear],
        format!(
            "{custom:?} — `ItemUseAnimation`'s third constructor argument. For              these the resting arm transform is skipped up front and applied              *after* the pose; for the other nine it is applied first and the              pose refines it"
        ),
    );

    // `scaledUsageTime` is `remaining / duration`, and remaining counts
    // **down** — so it starts at one and ends at zero, and `1 - scaled^27`
    // does the opposite. The exponent is what makes the item leave the rest
    // pose almost immediately and stay aside for the whole use.
    let rest = at(&pose(UseAnim::None, 5, 32), false).unwrap();
    let start = at(&pose(UseAnim::Eat, 32, 32), false).unwrap();
    let two_in = at(&pose(UseAnim::Eat, 30, 32), false).unwrap();
    let end = at(&pose(UseAnim::Eat, 1, 32), false).unwrap();
    c.record(
        "u2.eating_leaves_the_rest_pose_at_once_and_stays_aside",
        (start - rest).length() < 0.02
            && (two_in - rest).length() > 0.4
            && (end - rest).length() > 0.4,
        format!(
            "at the first tick the item is {:.3} from rest, two ticks in {:.2},              and at the last tick still {:.2}. `1 - scaled^27` climbs to one              within a couple of ticks and holds — a gentler exponent would ease              the food aside over the whole use instead of snapping it there",
            (start - rest).length(),
            (two_in - rest).length(),
            (end - rest).length()
        ),
    );

    // The brush cycles on its own ten ticks, not on the use's duration.
    let a = at(&pose(UseAnim::Brush, 100, 200), false).unwrap();
    let b = at(&pose(UseAnim::Brush, 110, 200), false).unwrap();
    let mid = at(&pose(UseAnim::Brush, 105, 200), false).unwrap();
    c.record(
        "u3.the_brush_sweeps_on_a_ten_tick_loop_of_its_own",
        (a - b).length() < 1e-5 && (a - mid).length() > 0.01,
        format!(
            "ten ticks apart the pose repeats exactly ({:.5} apart) while five              ticks in it differs ({:.3}). Brushing runs as long as you hold it,              so the sweep cannot key on progress through a duration the way              eating does — `remaining % 10` is what makes it cycle",
            (a - b).length(),
            (a - mid).length()
        ),
    );

    // The bow's draw saturates, and the shake only appears past a tenth.
    let drawn = |ticks: i32| at(&pose(UseAnim::Bow, 72000 - ticks, 72000), false).unwrap();
    c.record(
        "u4.the_bow_draw_saturates_after_twenty_ticks",
        (drawn(20) - drawn(60)).length() < 0.02 && (drawn(0) - drawn(20)).length() > 0.005,
        format!(
            "twenty ticks and sixty are {:.4} apart, while nought and twenty are              {:.4}. `power` is clamped to one, so holding a fully drawn bow              longer changes nothing — only the sub-4mm strain shake keeps moving",
            (drawn(20) - drawn(60)).length(),
            (drawn(0) - drawn(20)).length()
        ),
    );

    // A shield is excepted from the BLOCK pose, and scoping hides the hand.
    let shield_posed = at(&pose(UseAnim::Block, 10, 72000), true).unwrap();
    let other_posed = at(&pose(UseAnim::Block, 10, 72000), false).unwrap();
    c.record(
        "u5.a_shield_is_excepted_and_a_spyglass_hides_the_hand",
        (shield_posed - rest).length() < 1e-6
            && (other_posed - rest).length() > 0.1
            && at(&pose(UseAnim::Spyglass, 10, 72000), false).is_none(),
        format!(
            "a shield stays at rest ({:.4} from it) because it carries its own              display transform for the context and would be posed twice;              anything else blocking moves {:.2}. Scoping returns no matrix at              all — vanilla guards the whole of `submitArmWithItem` on              `!isScoping()`, so the hand is not drawn rather than drawn wrongly",
            (shield_posed - rest).length(),
            (other_posed - rest).length()
        ),
    );

    // -- STAB, the spear rig -----------------------------------------------
    //
    // A different shape from WHACK, not a retuning: three easings over three
    // overlapping windows, combined by *difference*, so the spear draws back
    // before it thrusts.
    let stab_at = |a: f32| {
        rewo_gpu::hand::item_hand_kind(Arm::Right, 0.0, a, SwingKind::Stab)
            .transform_point3(glam::Vec3::ZERO)
    };
    let whack_at = |a: f32| {
        rewo_gpu::hand::item_hand_kind(Arm::Right, 0.0, a, SwingKind::Whack)
            .transform_point3(glam::Vec3::ZERO)
    };
    let rest_pt = stab_at(0.0);
    c.record(
        "s1.the_spear_rig_differs_from_the_sweep_through_the_middle",
        (1..8)
            .map(|i| i as f32 / 8.0)
            .all(|a| (stab_at(a) - whack_at(a)).length() > 0.02)
            && (stab_at(0.0) - whack_at(0.0)).length() < 1e-6
            && (stab_at(1.0) - whack_at(1.0)).length() < 1e-3,
        format!(
            "at half way the spear is at {:?} and the sweep at {:?}, and the two              coincide at both ends — every swing returns to rest, so a witness              demanding they differ *everywhere* is wrong at `attack == 1`.              Sharing one rig would make a trident swing like a sword, which is              what this milestone shipped before the type was read",
            stab_at(0.5),
            whack_at(0.5)
        ),
    );

    // The wind-up: `starting - middle` is negative once `outBack` overshoots,
    // so the spear moves *back* before it goes forward. z is toward the viewer.
    let early = stab_at(0.04);
    let mid = stab_at(0.3);
    c.record(
        "s2.the_spear_draws_back_before_it_thrusts",
        early.z > rest_pt.z && mid.z < early.z,
        format!(
            "z goes {:.4} at rest, {:.4} early, {:.4} mid — toward the viewer, then              away. The pose is built from *differences* of three easings over              overlapping windows, which is what produces the retraction; reading              any one of them alone gives a monotonic slide",
            rest_pt.z, early.z, mid.z
        ),
    );

    // -- the bare arm ------------------------------------------------------
    //
    // The skin occupies a quarter of this atlas, so a UV that escaped its rect
    // is unmistakable.
    let geo = rewo_gpu::hand::ArmGeometry {
        skin_uv: [0.25, 0.5, 0.25, 0.25],
        slim: false,
    };
    let bare = |main_hand: bool| {
        build_vertices(
            Mat4::IDENTITY,
            &[HandDraw {
                arm: Arm::Right,
                item: None,
                attack: 0.0,
                inverse_height: 0.0,
                swings: SwingKind::Whack,
                main_hand,
                using: None,
                is_shield: false,
            }],
            &|_| Some([0.0, 0.0, 1.0, 1.0]),
            Some(&geo),
        )
    };
    let arm_verts = bare(true);
    c.record(
        "a1.the_bare_arm_draws_for_the_main_hand_only",
        !arm_verts.is_empty() && bare(false).is_empty(),
        format!(
            "{} verts for the main hand, none for the off hand.              `submitArmWithItem` renders the player's arm only when `isMainHand`              — an empty off-hand shows nothing at all, not a second arm",
            arm_verts.len()
        ),
    );

    // The model's UVs are texels, and normalising them is what keeps the arm
    // inside its rect. Without the divide they land far outside, where the
    // sampler clamps to a transparent edge and the arm vanishes — which is
    // precisely how this shipped broken once.
    let outside = arm_verts
        .iter()
        .filter(|v| {
            v.uv[0] < geo.skin_uv[0] - 1e-4
                || v.uv[0] > geo.skin_uv[0] + geo.skin_uv[2] + 1e-4
                || v.uv[1] < geo.skin_uv[1] - 1e-4
                || v.uv[1] > geo.skin_uv[1] + geo.skin_uv[3] + 1e-4
        })
        .count();
    let span = arm_verts
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
            (lo.min(v.uv[0]).min(v.uv[1]), hi.max(v.uv[0]).max(v.uv[1]))
        });
    c.record(
        "a2.the_arms_texel_uvs_are_normalised_into_the_skins_rect",
        outside == 0 && span.1 - span.0 > 0.01,
        format!(
            "{outside} of {} vertices fall outside the skin's rect, and the UVs              span {:.3}..{:.3} — the model publishes texels (an arm's are 16..56              of a 64 px skin), so they divide by the skin size before the remap.              Skipping that divide sends them to 16..56 in a 0..1 atlas, and the              arm renders as nothing",
            arm_verts.len(),
            span.0,
            span.1
        ),
    );

    // The arm's rest pose carries the fixed tilt, not the model's animation.
    let tilted = arm_verts
        .iter()
        .any(|v| v.pos[0].abs() > 1e-3 && v.pos[1].abs() > 1e-3);
    c.record(
        "a3.the_arm_is_posed_and_placed_rather_than_left_at_the_origin",
        tilted
            && arm_verts.iter().all(|v| v.pos[2] < 0.0),
        format!(
            "every vertex sits in front of the eye (negative z) and off-axis.              `renderHand` resets the part's pose and sets a fixed `zRot` of              ±0.1 rad, so the first-person arm is the rest pose plus one nudge —              not whatever the body animation left it at"
        ),
    );

    // The draw-time mirror negates exactly three components.
    let t = DisplayTransform {
        rotation: [10.0, 20.0, 30.0],
        translation: [0.1, 0.2, 0.3],
        scale: [0.5, 0.6, 0.7],
    };
    let r = display_transform(&t, false);
    let l = display_transform(&t, true);
    let pr = r.transform_point3(glam::Vec3::splat(0.5));
    let pl = l.transform_point3(glam::Vec3::splat(0.5));
    c.record(
        "p8.the_draw_time_mirror_negates_x_translation_and_yz_rotation",
        (pr.x + pl.x).abs() < 1e-6 && (pr.y - pl.y).abs() < 1e-6 && r != l,
        format!(
            "the model's centre maps to x {:.4} and {:.4} under the two, with y \
             unchanged. `ItemTransform.apply` mirrors whichever transform was \
             selected, including one that arrived through the absent-left fallback",
            pr.x, pl.x
        ),
    );
}

// -- 3. the pixels --------------------------------------------------------------

/// Count magenta pixels in a window.
///
/// Magenta because nothing else in the frame can be: the clear is black and
/// the gate draws no world. That is the whole point — see the module header.
fn magenta(img: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let mut n = 0;
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            let i = ((y * W + x) * 4) as usize;
            if img[i] > 150 && img[i + 2] > 150 && img[i + 1] < 90 {
                n += 1;
            }
        }
    }
    n
}

fn check_pixels(c: &mut Checker, args: &HandshotArgs) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[handshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("handshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    // A 16x16 magenta atlas — one texture, fully opaque.
    let atlas: Vec<u8> = (0..16 * 16).flat_map(|_| [255u8, 0, 255, 255]).collect();
    let r = (|| -> Result<(), String> {
        wr.init_hand(&mut gpu, &atlas, 16, 16)?;
        let ring = OverlayRing::default();
        let draw = OverlayDraw {
            samples_ms: &ring.data,
            head: ring.head(),
            scale_ms: 20.0,
            origin: [-4000.0, -4000.0],
            size: [8.0, 8.0],
        };
        let vp = projection().to_cols_array_2d();
        let item = cube_item(block_transform(), block_transform());

        let mut shot = |gpu: &mut Gpu,
                        off: &mut Offscreen,
                        wr: &mut WorldRenderer,
                        v: &[rewo_gpu::gui_item::GuiItemVertex]|
         -> Result<Vec<u8>, String> {
            wr.set_hand(gpu, v, vp)?;
            off.render(gpu, Some((&mut *wr, vp)), &draw, CLEAR)?;
            off.read_rgba(gpu)
        };

        let empty = shot(&mut gpu, &mut off, &mut wr, &[])?;
        let rest_verts = draw_one(Arm::Right, &item, 0.0, 0.0);
        let rest = shot(&mut gpu, &mut off, &mut wr, &rest_verts)?;
        if let Some(d) = &args.out_dir {
            std::fs::create_dir_all(d).map_err(|e| format!("out-dir: {e}"))?;
            let _ = off.save_png(&mut gpu, &d.join("handshot.png"));
        }

        c.record(
            "g1.an_empty_hand_draws_nothing",
            magenta(&empty, 0, 0, W, H) == 0,
            "no magenta anywhere, so every count below is the hand and nothing else",
        );

        // The item lands inside the rect the placement layer predicted.
        let (bx0, by0, bx1, by1) = screen_bbox(&rest_verts);
        let inside = magenta(
            &rest,
            bx0.max(0.0) as u32,
            by0.max(0.0) as u32,
            (bx1 + 1.0).max(0.0) as u32,
            (by1 + 1.0).max(0.0) as u32,
        );
        let total = magenta(&rest, 0, 0, W, H);
        c.record(
            "g2.every_drawn_pixel_falls_inside_the_predicted_rect",
            total > 500 && inside == total,
            format!(
                "{total} magenta px, all {inside} of them within \
                 ({bx0:.0}, {by0:.0})..({bx1:.0}, {by1:.0}) — the rect the CPU said, \
                 so the pass and the builder agree"
            ),
        );

        c.record(
            "g3.the_item_sits_in_the_lower_right",
            magenta(&rest, W / 2, H / 2, W, H) > magenta(&rest, 0, 0, W / 2, H / 2) * 10 + 100,
            format!(
                "{} px in the lower-right quadrant against {} in the upper-left — the \
                 right hand's `ITEM_POS_X` is positive and `ITEM_POS_Y` negative",
                magenta(&rest, W / 2, H / 2, W, H),
                magenta(&rest, 0, 0, W / 2, H / 2),
            ),
        );

        // The left hand is the mirror.
        let left_verts = draw_one(Arm::Left, &item, 0.0, 0.0);
        let left = shot(&mut gpu, &mut off, &mut wr, &left_verts)?;
        c.record(
            "g4.the_left_hand_renders_on_the_other_side",
            magenta(&left, 0, H / 2, W / 2, H) > 500
                && magenta(&left, W / 2, H / 2, W, H) < 100,
            format!(
                "{} px lower-left, {} lower-right — the reflection of g3",
                magenta(&left, 0, H / 2, W / 2, H),
                magenta(&left, W / 2, H / 2, W, H),
            ),
        );

        // A swing displaces what is drawn: the mutation partner for g2.
        let swung = shot(&mut gpu, &mut off, &mut wr, &draw_one(Arm::Right, &item, 0.4, 0.0))?;
        let moved = (0..(W * H) as usize)
            .filter(|i| rest[i * 4..i * 4 + 3] != swung[i * 4..i * 4 + 3])
            .count();
        c.record(
            "g5.a_swing_changes_the_frame",
            moved > 2000,
            format!("{moved} px differ between rest and a swing at 0.4"),
        );

        // Depth within the pass: a nearer copy occludes a further one.
        let near_item = cube_item(
            DisplayTransform {
                rotation: [0.0, 45.0, 0.0],
                translation: [0.0, 0.0, 0.15],
                scale: [0.4; 3],
            },
            block_transform(),
        );
        let mut both = draw_one(Arm::Right, &item, 0.0, 0.0);
        both.extend(draw_one(Arm::Right, &near_item, 0.0, 0.0));
        let layered = shot(&mut gpu, &mut off, &mut wr, &both)?;
        let near_only = shot(&mut gpu, &mut off, &mut wr, &draw_one(Arm::Right, &near_item, 0.0, 0.0))?;
        c.record(
            "g6.the_pass_depth_sorts_within_itself",
            magenta(&layered, 0, 0, W, H) >= magenta(&near_only, 0, 0, W, H),
            format!(
                "two overlapping copies cover {} px against the nearer alone at {} — \
                 the pass owns a depth buffer, so a block item's own faces sort \
                 rather than drawing in submission order",
                magenta(&layered, 0, 0, W, H),
                magenta(&near_only, 0, 0, W, H),
            ),
        );

        // And clearing the list restores the empty frame exactly.
        let again = shot(&mut gpu, &mut off, &mut wr, &[])?;
        c.record(
            "g7.clearing_the_hand_restores_the_empty_frame",
            again == empty,
            "byte-identical, so no witness above read a stale buffer",
        );
        Ok(())
    })();
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    r
}
