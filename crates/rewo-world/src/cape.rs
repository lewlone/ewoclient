//! The vanilla cape's three angles (M60).
//!
//! A transcription of `AvatarRenderer.extractCapeState`, kept here rather
//! than in the renderer because it is arithmetic over entity state and
//! nothing else — no GPU, no atlas, no model. The renderer turns these three
//! numbers into a rotation; `capeshot` grades them directly.
//!
//! **Everything is in degrees**, as vanilla's `AvatarRenderState` fields are;
//! the conversion to radians happens where the rotation is built.
//!
//! The cape is driven by the *gap* between the player and their lagging
//! cloak anchor ([`crate::entities::EntityState::cloak_pos`]): standing still
//! the anchor catches up and the cape hangs flat, and accelerating opens a
//! gap that lifts it. So there is no "is the player moving" test anywhere —
//! the lag is the whole mechanism.

/// `AvatarRenderState`'s three cape angles, in degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CapeAngles {
    /// Lift from vertical motion, plus the local player's walk bob.
    pub flap: f32,
    /// Lift from motion **along** the facing. Clamped at 0 below, so the
    /// cape never leans forward however hard a player walks backwards.
    pub lean: f32,
    /// Sideways sway, from motion across the facing.
    pub lean2: f32,
}

/// `LivingEntity.fallFlyingScale()` —
/// `clamp(fallFlyingTimeInTicks² / 100, 0, 1)`.
///
/// Squared, so it reaches 1 at 10 ticks and the elytra suppression of
/// [`CapeAngles::lean`] is complete half a second into a glide.
pub fn fall_flying_scale(fall_flying_time_in_ticks: f32) -> f32 {
    (fall_flying_time_in_ticks * fall_flying_time_in_ticks / 100.0).clamp(0.0, 1.0)
}

/// `AvatarRenderer.extractCapeState`.
///
/// * `cloak` — `getInterpolatedCloak{X,Y,Z}(partialTicks)`
/// * `pos` — `Mth.lerp(partialTicks, entity.{x,y,z}o, entity.get{X,Y,Z}())`
/// * `y_body_rot` — `Mth.rotLerp(partialTicks, yBodyRotO, yBodyRot)`, degrees
/// * `fall_flying_time_in_ticks` — `getFallFlyingTicks() + partialTicks`
/// * `bob` — `getInterpolatedBob(partialTicks)`
/// * `walk_distance` — `getInterpolatedWalkDistance(partialTicks)`
///
/// The last two are **always zero for a remote player**, and not for the
/// same reason. `walkDist`'s only writer chain is
/// `LocalPlayer.move → addWalkedDistance → ClientAvatarState.addWalkDistance`,
/// so a remote player's stays 0 forever and `sin(0 · 6) · 32 · bob` is 0
/// whatever `bob` is. `bob` itself is **not** zero for remotes —
/// `RemotePlayer.tick` calls `updateBob()` — so gating the walk term on
/// `bob != 0` would be wrong even though it happens to look equivalent.
pub fn cape_angles(
    cloak: [f64; 3],
    pos: [f64; 3],
    y_body_rot: f32,
    fall_flying_time_in_ticks: f32,
    bob: f32,
    walk_distance: f32,
) -> CapeAngles {
    let delta_x = cloak[0] - pos[0];
    let delta_y = cloak[1] - pos[1];
    let delta_z = cloak[2] - pos[2];
    let forward_x = (y_body_rot.to_radians()).sin() as f64;
    let forward_z = -(y_body_rot.to_radians()).cos() as f64;

    // The clamp lands **before** the walk-bob add, so a walking local player
    // legitimately exceeds 32°. Folding them the other way would cap the
    // walk sway that is the whole point of the term.
    let mut flap = (delta_y as f32) * 10.0;
    flap = flap.clamp(-6.0, 32.0);

    // Scale by the elytra term, then clamp — not the reverse. `capeLean`'s
    // lower bound is 0, not -150: motion opposite the facing produces a
    // negative dot and is clamped away, so the cape only ever lifts behind.
    let mut lean = ((delta_x * forward_x + delta_z * forward_z) as f32) * 100.0;
    lean *= 1.0 - fall_flying_scale(fall_flying_time_in_ticks);
    lean = lean.clamp(0.0, 150.0);

    let mut lean2 = ((delta_x * forward_z - delta_z * forward_x) as f32) * 100.0;
    lean2 = lean2.clamp(-20.0, 20.0);

    flap += (walk_distance * 6.0).sin() * 32.0 * bob;
    CapeAngles { flap, lean, lean2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_player_hangs_flat() {
        let a = cape_angles([0.0; 3], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
        assert_eq!(a, CapeAngles::default());
    }

    #[test]
    fn the_lower_lean_clamp_is_zero_not_negative() {
        // Cloak *ahead* of the player (dot > 0 against forward) would lean
        // the cape forward; vanilla clamps that away entirely.
        // yBodyRot 0 → forward = (0, -1), so a cloak at -z is "ahead".
        let a = cape_angles([0.0, 0.0, -1.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
        assert_eq!(a.lean, 100.0, "behind → lifts, 1 block = 100 degrees");
        let b = cape_angles([0.0, 0.0, 1.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
        assert_eq!(b.lean, 0.0, "in front → clamped to flat, never negative");
        // The upper clamp needs a gap the easing could never open on its own.
        let c = cape_angles([0.0, 0.0, -2.0], [0.0; 3], 0.0, 0.0, 0.0, 0.0);
        assert_eq!(c.lean, 150.0, "and it saturates");
    }

    #[test]
    fn ten_elytra_ticks_suppress_lean_entirely() {
        let a = cape_angles([0.0, 0.0, -1.0], [0.0; 3], 0.0, 10.0, 0.0, 0.0);
        assert_eq!(fall_flying_scale(10.0), 1.0);
        assert_eq!(a.lean, 0.0);
    }

    #[test]
    fn the_flap_clamp_lands_before_the_walk_bob_add() {
        // deltaY 10 → raw flap 100, clamped to 32; the bob term then pushes
        // it past the clamp, which is exactly what vanilla's ordering does.
        let walk = std::f32::consts::FRAC_PI_2 / 6.0; // sin(walk·6) == 1
        let a = cape_angles([0.0, 10.0, 0.0], [0.0; 3], 0.0, 0.0, 0.5, walk);
        assert!((a.flap - (32.0 + 16.0)).abs() < 1e-4, "flap = {}", a.flap);
    }

    #[test]
    fn a_remote_player_carries_no_walk_term_even_with_bob() {
        // walkDist is 0 for every remote player, so the term vanishes
        // through sin(0) — `bob` is deliberately large here to show it is
        // not the thing doing the vanishing.
        let a = cape_angles([0.0, 0.1, 0.0], [0.0; 3], 0.0, 0.0, 1.0, 0.0);
        assert_eq!(a.flap, 1.0, "just the deltaY term");
    }
}
