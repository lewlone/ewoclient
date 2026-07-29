//! The local player's rotation, as the server writes it (M76).
//!
//! Two clientbound packets turn your head, and neither was decoded before this
//! milestone: `player_rotation` (73) sets yaw/pitch directly, and
//! `player_look_at` (71) — `/teleport … facing` — computes them from a point.
//! Their positional twin `player_position` (72) has worked since M3, which is
//! exactly what made the gap hard to see: a server that *moves* you works, so
//! the natural diagnosis for one that *turns* you is "teleports work, so it
//! isn't the teleport path".
//!
//! This module is the player-state half — the arithmetic vanilla runs between
//! the wire and `LocalPlayer.yRot`/`xRot`. The decode is
//! [`rewo_net::player_rotation`], mirroring the split M75 used for the
//! abilities (`rewo_net::abilities` reads the bytes, `rewo_world::abilities`
//! moves the player).
//!
//! # `player_rotation` is **not** the `RelativeMovement` bitfield
//!
//! The obvious guess — and the one `REWO_PACKET_COVERAGE.md` §3 wrote down —
//! is that it carries the same packed `Set<Relative>` int that
//! `ClientboundPlayerPositionPacket` does. It does not.
//! `ClientboundPlayerRotationPacket` is
//!
//! ```text
//! record(float yRot, boolean relativeY, float xRot, boolean relativeX)
//! ```
//!
//! four `StreamCodec.composite` fields **interleaved**, ten fixed bytes, with
//! the two flags as plain `ByteBufCodecs.BOOL`s sitting *after* the float each
//! one qualifies. There is no `Relative.SET_STREAM_CODEC` anywhere in the
//! packet. The bitfield only appears one layer up, where `handleRotatePlayer`
//! calls `Relative.rotation(relativeY, relativeX)` to build the set that
//! [`apply_relative_rotation`] consumes — so the *semantics* are shared with
//! the positional teleport while the *wire layout* is not. Reading it as an
//! `i32` mask would consume the yaw's four bytes as the mask and then read the
//! next packet's bytes as a float.
//!
//! Note also which axis each flag names: `relativeY` qualifies `yRot`, the
//! rotation **about** Y — the yaw. It is not a position axis, and the packet
//! has no Z counterpart.
//!
//! # The clamp is applied twice, and only the second one can bite
//!
//! `PositionMoveRotation.calculateAbsolute` clamps the pitch to ±90 *after*
//! adding the relative base, and then `Entity.setXRot` clamps it again — but
//! `setXRot`'s is `Math.clamp(xRot % 360.0F, -90, 90)`, a **modulo then** a
//! clamp. For `player_rotation` the second is idempotent because the first has
//! already landed the value inside the range. For [`look_at`] the first never
//! runs, so `set_x_rot` is the only clamp, and its `% 360` is the part that
//! separates it from a plain clamp.
//!
//! The yaw gets neither: `setYRot` is a bare assignment behind a finiteness
//! guard. A server sending yaw `720.0` non-relative leaves the player at
//! `720.0`, and that value goes back out on the wire unwrapped. Wrapping it —
//! which looks like tidying — is a divergence.
//!
//! # A non-finite rotation is **discarded**, not clamped
//!
//! Both setters guard on `Float.isFinite` and, when it fails, log and return
//! **without writing**. So a NaN pitch leaves the previous pitch standing. That
//! is reachable: `Mth.clamp(NaN, -90, 90)` is `NaN` (its first test is
//! `value < min`, which is false for NaN, so it falls through to `Math.min`),
//! so a NaN on the wire survives `calculateAbsolute` and is stopped only here.
//!
//! # `Mth.atan2`, not `Math.atan2`
//!
//! `Entity.lookAt` calls vanilla's own [`atan2`] — a 257-entry
//! `asin`/`cos` table plus a Quake-style [`fast_inv_sqrt`] and a cubic
//! correction, accurate to roughly 1e-6 rad rather than to the ULP. Substituting
//! the platform `atan2` is the same class of mistake M12 recorded for
//! `Mth.sin`: it agrees to eyeball precision everywhere and is not the function
//! vanilla evaluated.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/ClientboundPlayerRotationPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundPlayerLookAtPacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleRotatePlayer`, `handleLookAt`, `handleMovePlayer`
//! - `net/minecraft/world/entity/PositionMoveRotation.java` — `calculateAbsolute`
//! - `net/minecraft/world/entity/Relative.java` — `rotation`
//! - `net/minecraft/world/entity/Entity.java` — `lookAt`, `setXRot`, `setYRot`
//! - `net/minecraft/commands/arguments/EntityAnchorArgument.java` — `Anchor`
//! - `net/minecraft/util/Mth.java` — `atan2`, `fastInvSqrt`, `wrapDegrees`, `clamp`

/// `EntityAnchorArgument.Anchor`, in **declaration** order.
///
/// The wire form is `FriendlyByteBuf.readEnum`, which is
/// `values()[readVarInt()]` — an array index, so an out-of-range ordinal is an
/// `ArrayIndexOutOfBoundsException`, i.e. a decode **error**. That is the
/// first of the three enum conventions this codebase has now met: `readEnum`
/// errors, `ByIdMap.continuous(…, ZERO)` returns the zero value (M65's
/// `DisplaySlot`), and `ByIdMap.continuous(…, WRAP)` takes `Math.floorMod`
/// (M74's `Difficulty`, where a negative id is legal). Only the decompile
/// distinguishes them, and all three are reachable with a two-value enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Feet = 0,
    Eyes = 1,
}

impl Anchor {
    /// The two values in `values()` order, which is what `readEnum` indexes.
    pub const VALUES: [Anchor; 2] = [Anchor::Feet, Anchor::Eyes];

    /// `readEnum`'s lookup. `None` is vanilla's `ArrayIndexOutOfBoundsException`
    /// — the caller turns it into a rejected packet, never into a default.
    pub fn from_ordinal(ordinal: i32) -> Option<Anchor> {
        usize::try_from(ordinal)
            .ok()
            .and_then(|i| Self::VALUES.get(i).copied())
    }

    /// `Anchor.apply(entity)`: `FEET` is the entity's position verbatim,
    /// `EYES` raises it by that entity's eye height.
    ///
    /// The eye height is a parameter rather than a constant because vanilla
    /// reads it off the *entity* — `Entity.getEyeHeight()` is
    /// `dimensions.eyeHeight()`, which differs per type. For the local player
    /// it is [`crate::physics::EYE_HEIGHT`].
    pub fn apply(self, pos: [f64; 3], eye_height: f64) -> [f64; 3] {
        match self {
            Anchor::Feet => pos,
            Anchor::Eyes => [pos[0], pos[1] + eye_height, pos[2]],
        }
    }
}

// ───────────────────────────────────────────────────────────── vanilla `Mth`

/// `Mth.wrapDegrees(float)` — into `[-180, 180)`.
///
/// Java's `%` on floats is a *truncated* remainder (the sign follows the
/// dividend), which Rust's `%` matches exactly, so the two halves that follow
/// are the whole of it.
pub fn wrap_degrees(angle: f32) -> f32 {
    let mut a = angle % 360.0;
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// `Mth.clamp(float, float, float)` = `value < min ? min : Math.min(value, max)`.
///
/// Written out rather than deferred to `f32::clamp` for one reason: the NaN
/// path. `f32::clamp` **panics** if `min > max` and is otherwise NaN-preserving,
/// which happens to agree here — but the property that matters downstream is
/// that a NaN survives this call to reach [`set_x_rot`]'s finiteness guard, and
/// that is a consequence of the exact expression, not of the name `clamp`.
pub fn clamp_f32(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else {
        // `Math.min` propagates NaN, which is the branch this exists for.
        if value.is_nan() {
            value
        } else if value < max {
            value
        } else {
            max
        }
    }
}

/// `Mth.fastInvSqrt(double)` — one Newton step off the magic-constant seed.
///
/// `i >> 1` is Java's **arithmetic** shift on a signed `long`, so the sign bit
/// replicates; `i64 >> 1` in Rust is the same operation. A `u64 >> 1` would be
/// a logical shift and would differ for any negative-bit-pattern input.
pub fn fast_inv_sqrt(x: f64) -> f64 {
    let xhalf = 0.5 * x;
    let i = x.to_bits() as i64;
    let i = 6_910_469_410_427_058_090_i64 - (i >> 1);
    let x = f64::from_bits(i as u64);
    x * (1.5 - xhalf * x * x)
}

/// `Mth.FRAC_BIAS` — `Double.longBitsToDouble(4805340802404319232L)`.
///
/// The bias that turns `y ∈ [0, 1]` into a table index: adding it forces the
/// exponent so that the low bits of the raw representation *are* the index.
const FRAC_BIAS: f64 = f64::from_bits(4_805_340_802_404_319_232_u64);

/// `Mth.LUT_SIZE` — 257 entries, so index 256 is `asin(1.0)` exactly.
const LUT_SIZE: usize = 257;

/// `Mth.ASIN_TAB` / `Mth.COS_TAB`, built exactly as vanilla's static
/// initialiser builds them:
///
/// ```java
/// for (int ind = 0; ind < 257; ind++) {
///    double v = ind / 256.0;
///    double asinv = Math.asin(v);
///    COS_TAB[ind] = Math.cos(asinv);
///    ASIN_TAB[ind] = asinv;
/// }
/// ```
///
/// `libm` rather than the platform math for the same reason M12 took the
/// dependency: it is fdlibm, which is what Java's `StrictMath` is, and HotSpot
/// does **not** intrinsify `asin` — `Math.asin` delegates straight to
/// `StrictMath.asin`, so that column is exact.
///
/// `Math.cos` *is* intrinsified on x86-64 and may therefore differ from
/// `libm::cos` by up to 1 ULP. That is a ~1e-16 relative difference in
/// `COS_TAB`, four orders of magnitude below [`atan2`]'s own ~1e-6
/// approximation error, and it is stated here rather than papered over: this
/// module claims to reproduce `Mth.atan2`'s *algorithm* to the operation, not
/// its result to the bit.
struct AtanTables {
    asin: [f64; LUT_SIZE],
    cos: [f64; LUT_SIZE],
}

fn atan_tables() -> &'static AtanTables {
    use std::sync::OnceLock;
    static TABLES: OnceLock<AtanTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut asin = [0.0f64; LUT_SIZE];
        let mut cos = [0.0f64; LUT_SIZE];
        for ind in 0..LUT_SIZE {
            let v = ind as f64 / 256.0;
            let asinv = libm::asin(v);
            cos[ind] = libm::cos(asinv);
            asin[ind] = asinv;
        }
        AtanTables { asin, cos }
    })
}

/// `Mth.atan2(double y, double x)` — vanilla's table-driven approximation.
///
/// Transcribed statement for statement. Two details that a rewrite would
/// naturally "improve" and must not:
///
/// * `int index = (int) Double.doubleToRawLongBits(yp)` is a **narrowing** cast
///   that keeps the low 32 bits, which is what `as u32` does to a `u64` here.
///   Widening it, or masking to 9 bits, changes which entry is read.
/// * the final `theta` corrections are applied in the order `steep`, `negX`,
///   `negY`, and they do not commute.
///
/// **One deliberate deviation, at the range check.** For a finite argument the
/// normalisation puts `y ∈ [0, 1]` and the index lands in `0..=256`; for a
/// non-finite one Java throws `ArrayIndexOutOfBoundsException`, which its netty
/// pipeline turns into a disconnect. A panic is not an acceptable failure mode
/// for a Rust packet handler, so the index saturates instead — and it is
/// unreachable from [`look_at`], which declines a non-finite delta before
/// calling here precisely so that this clause never decides anything.
pub fn atan2(y: f64, x: f64) -> f64 {
    let mut y = y;
    let mut x = x;
    let d2 = x * x + y * y;
    if d2.is_nan() {
        return f64::NAN;
    }
    let neg_y = y < 0.0;
    if neg_y {
        y = -y;
    }
    let neg_x = x < 0.0;
    if neg_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    let rinv = fast_inv_sqrt(d2);
    x *= rinv;
    y *= rinv;
    let yp = FRAC_BIAS + y;
    let index = (yp.to_bits() as u32) as usize;
    let index = index.min(LUT_SIZE - 1);
    let tables = atan_tables();
    let phi = tables.asin[index];
    let c_phi = tables.cos[index];
    let s_phi = yp - FRAC_BIAS;
    let sd = y * c_phi - x * s_phi;
    let d = (6.0 + sd * sd) * sd * 0.166_666_666_666_666_66;
    let mut theta = phi + d;
    if steep {
        theta = std::f64::consts::FRAC_PI_2 - theta;
    }
    if neg_x {
        theta = std::f64::consts::PI - theta;
    }
    if neg_y {
        theta = -theta;
    }
    theta
}

// ─────────────────────────────────────────────────────── the vanilla setters

/// `Entity.setYRot(float)`.
///
/// A bare assignment behind a finiteness guard — **no wrap, no clamp**. Returns
/// whether the write happened, so a caller can witness the discard.
pub fn set_y_rot(current: &mut f32, y_rot: f32) -> bool {
    if !y_rot.is_finite() {
        // `Util.logAndPauseIfInIde("Invalid entity rotation: …, discarding.")`
        return false;
    }
    *current = y_rot;
    true
}

/// `Entity.setXRot(float)` = `Math.clamp(xRot % 360.0F, -90.0F, 90.0F)`.
///
/// The `% 360` before the clamp is the whole difference from a plain clamp, and
/// it is **not** a no-op: `370.0` reduces to `10.0` where a bare clamp would
/// give `90.0`. It matters for [`look_at`], whose result is only bounded by
/// `wrap_degrees` to ±180 before arriving here.
///
/// `Math.clamp` rather than `Mth.clamp`, which for the finite values that reach
/// this point are the same function.
pub fn set_x_rot(current: &mut f32, x_rot: f32) -> bool {
    if !x_rot.is_finite() {
        return false;
    }
    *current = (x_rot % 360.0).clamp(-90.0, 90.0);
    true
}

// ────────────────────────────────────────────────── `player_rotation` (73)

/// The rotation half of `PositionMoveRotation.calculateAbsolute`, followed by
/// `Entity.setYRot` / `setXRot` — i.e. the whole body of `handleRotatePlayer`
/// after `Relative.rotation(…)` has turned the two booleans into a set.
///
/// The positional and delta halves are omitted because vanilla's own call
/// makes them identities: `handleRotatePlayer` passes
/// `currentValues.withRotation(yRot, xRot)` as the *change*, so the change's
/// position and `deltaMovement` are the player's own, and neither `X`/`Y`/`Z`
/// nor any `DELTA_*` is in the set — `calculateAbsolute` therefore writes the
/// position and velocity back unchanged, and `handleRotatePlayer` does not read
/// them anyway. This is a simplification of the *call*, not of the function.
///
/// Note the composition base: `PositionMoveRotation.of(player)` reads the
/// player's **current** rotation, not the last value sent to the server. A
/// relative rotation therefore composes with whatever the mouse has done since
/// the last outbound packet.
///
/// Returns `(wrote_yaw, wrote_pitch)` — either can be false when the packet
/// carries a non-finite value.
pub fn apply_relative_rotation(
    yaw: &mut f32,
    pitch: &mut f32,
    y_rot: f32,
    relative_y: bool,
    x_rot: f32,
    relative_x: bool,
) -> (bool, bool) {
    // `float offsetYRot = relatives.contains(Y_ROT) ? source.yRot : 0.0F;`
    let offset_y_rot = if relative_y { *yaw } else { 0.0 };
    let offset_x_rot = if relative_x { *pitch } else { 0.0 };
    // `float absoluteYRot = offsetYRot + change.yRot;`  — no clamp, no wrap.
    let absolute_y_rot = offset_y_rot + y_rot;
    // `float absoluteXRot = Mth.clamp(offsetXRot + change.xRot, -90.0F, 90.0F);`
    let absolute_x_rot = clamp_f32(offset_x_rot + x_rot, -90.0, 90.0);
    // Vanilla's order is yaw then pitch. Nothing here reads the other, so the
    // order is not load-bearing — it is kept to match the source.
    let wrote_yaw = set_y_rot(yaw, absolute_y_rot);
    let wrote_pitch = set_x_rot(pitch, absolute_x_rot);
    (wrote_yaw, wrote_pitch)
}

// ─────────────────────────────────────────────────── `player_look_at` (71)

/// `Entity.lookAt(Anchor, Vec3)` — the rotation that points `from` at `to`.
///
/// Returns `None` when any component of the delta is non-finite. Vanilla has no
/// such branch: it would feed the value to `Mth.atan2`, whose table index would
/// throw. Both outcomes leave the player's rotation untouched, which is the
/// property that matters; see [`atan2`]'s note on the saturating index.
///
/// # The Java casts are load-bearing and asymmetric
///
/// ```java
/// this.setXRot(Mth.wrapDegrees((float)(-(Mth.atan2(yd, sd) * 180.0F / (float)Math.PI))));
/// this.setYRot(Mth.wrapDegrees((float)(Mth.atan2(zd, xd) * 180.0F / (float)Math.PI) - 90.0F));
/// ```
///
/// * The pitch's `(float)` cast encloses the **negation**; the yaw's encloses
///   only the division, and `- 90.0F` is then a *float* subtraction. Moving
///   either bracket changes the rounding.
/// * `(float)Math.PI` is π rounded to `f32` and then widened back — not π. The
///   division is `f64` by a slightly-wrong π, which is a ~1e-8 relative error
///   that is *part of the specified behaviour*.
/// * The yaw's arguments are `(zd, xd)`, not `(xd, zd)`, and the `- 90.0F` is
///   what turns a maths-convention angle into Minecraft's south-zero yaw.
pub fn look_at(from: [f64; 3], to: [f64; 3]) -> Option<(f32, f32)> {
    let xd = to[0] - from[0];
    let yd = to[1] - from[1];
    let zd = to[2] - from[2];
    if !(xd.is_finite() && yd.is_finite() && zd.is_finite()) {
        return None;
    }
    let sd = (xd * xd + zd * zd).sqrt();
    // `(float)Math.PI` — π at f32 precision, widened for the f64 divide.
    let pi_f: f64 = std::f32::consts::PI as f64;
    let pitch = wrap_degrees((-(atan2(yd, sd) * 180.0 / pi_f)) as f32);
    let yaw = wrap_degrees(((atan2(zd, xd) * 180.0 / pi_f) as f32) - 90.0);
    Some((yaw, pitch))
}

/// `handleLookAt`'s effect: point the player at `to`, writing through the same
/// two setters `player_rotation` uses.
///
/// `from` is `fromAnchor.apply(player)` — the *player's* anchor, which is why
/// looking at a point from your feet and from your eyes give different pitches.
pub fn apply_look_at(yaw: &mut f32, pitch: &mut f32, from: [f64; 3], to: [f64; 3]) -> bool {
    let Some((new_yaw, new_pitch)) = look_at(from, to) else {
        return false;
    };
    // Vanilla's order: `setXRot` first, then `setYRot`. Neither reads the
    // other; kept to match `Entity.lookAt`.
    set_x_rot(pitch, new_pitch);
    set_y_rot(yaw, new_yaw);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── `Mth` primitives ────────────────────────────────────────────────

    /// MUTATION: swapping `>=` for `>` on the first branch, or dropping either
    /// branch, moves one of these. 180.0 and -180.0 are the two samples that
    /// sit *on* the bounds — a witness at 179/181 leaves the boundary
    /// comparison free, which is the failure the M75 battery recorded.
    #[test]
    fn wrap_degrees_is_half_open_at_180() {
        assert_eq!(wrap_degrees(0.0), 0.0);
        // Exactly 180 wraps *down* (`>=`), so the range is [-180, 180).
        assert_eq!(wrap_degrees(180.0), -180.0);
        // Exactly -180 does not wrap (`<`), so it is the range's own endpoint.
        assert_eq!(wrap_degrees(-180.0), -180.0);
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(360.0), 0.0);
        // Java's `%` is truncated, so a large negative reduces toward zero
        // before the branches, not away from it.
        assert_eq!(wrap_degrees(-540.0), -180.0);
    }

    /// `Mth.clamp` must let a NaN through, because [`set_x_rot`]'s finiteness
    /// guard is what stops it and a clamp that returned `min` would write -90.
    ///
    /// MUTATION: `if value.is_nan() { value }` → deleting that arm makes this
    /// return `max`, and `nan_pitch_is_discarded_not_clamped` below then sees
    /// a written pitch of 90 instead of the old value.
    #[test]
    fn clamp_passes_nan_through() {
        assert!(clamp_f32(f32::NAN, -90.0, 90.0).is_nan());
        assert_eq!(clamp_f32(-100.0, -90.0, 90.0), -90.0);
        assert_eq!(clamp_f32(100.0, -90.0, 90.0), 90.0);
        assert_eq!(clamp_f32(45.0, -90.0, 90.0), 45.0);
        // On the bounds themselves, where a `<` / `<=` slip would show.
        assert_eq!(clamp_f32(-90.0, -90.0, 90.0), -90.0);
        assert_eq!(clamp_f32(90.0, -90.0, 90.0), 90.0);
    }

    /// The table index really does stay inside the array for the whole finite
    /// domain, so [`atan2`]'s saturating clause is genuinely unreachable there.
    ///
    /// MUTATION: replacing `(yp.to_bits() as u32) as usize` with the full
    /// `to_bits() as usize` sends every index far past 256, the saturation
    /// pins them all to entry 256, and every angle below collapses.
    #[test]
    fn atan2_matches_the_platform_within_its_stated_accuracy() {
        // `Mth.atan2` is an approximation, so the assertion is a *bound*, not
        // equality — and the bound is two-sided: it must also not be exact,
        // or we would be testing that someone quietly swapped in `f64::atan2`.
        let mut worst = 0.0f64;
        let mut samples = 0u32;
        for i in -40..=40 {
            for j in -40..=40 {
                let (y, x) = (i as f64 * 0.37, j as f64 * 0.41);
                if x == 0.0 && y == 0.0 {
                    continue;
                }
                let diff = (atan2(y, x) - y.atan2(x)).abs();
                worst = worst.max(diff);
                samples += 1;
            }
        }
        assert!(samples > 6000, "sweep collapsed to {samples} samples");
        assert!(
            worst < 1.0e-5,
            "Mth.atan2 diverged from the platform by {worst}"
        );
        assert!(
            worst > 1.0e-12,
            "atan2 agreed with the platform to {worst} — this is not \
             vanilla's approximation, it is `f64::atan2`"
        );
    }

    /// The three quadrant corrections are applied in vanilla's order and do not
    /// commute. Checked at the four cardinals, where a swapped `negX`/`negY`
    /// gives a sign error that a first-quadrant-only sweep cannot see.
    ///
    /// MUTATION: reordering the `steep` / `neg_x` / `neg_y` blocks.
    #[test]
    fn atan2_cardinals() {
        let close = |a: f64, b: f64| (a - b).abs() < 1.0e-6;
        assert!(close(atan2(0.0, 1.0), 0.0));
        assert!(close(atan2(1.0, 0.0), std::f64::consts::FRAC_PI_2));
        assert!(close(atan2(0.0, -1.0), std::f64::consts::PI));
        assert!(close(atan2(-1.0, 0.0), -std::f64::consts::FRAC_PI_2));
        assert!(close(atan2(1.0, 1.0), std::f64::consts::FRAC_PI_4));
        assert!(close(atan2(-1.0, -1.0), -3.0 * std::f64::consts::FRAC_PI_4));
    }

    // ── the setters ─────────────────────────────────────────────────────

    /// MUTATION: dropping the `% 360.0` makes 370 clamp to 90 instead of
    /// reducing to 10. The sample is chosen past 360 precisely so the modulo
    /// is observable — a sample of 100 clamps to 90 either way.
    #[test]
    fn set_x_rot_reduces_before_clamping() {
        let mut p = 0.0f32;
        assert!(set_x_rot(&mut p, 370.0));
        assert_eq!(p, 10.0);
        // Past the clamp on the reduced value.
        let mut p = 0.0f32;
        assert!(set_x_rot(&mut p, 100.0));
        assert_eq!(p, 90.0);
        let mut p = 0.0f32;
        assert!(set_x_rot(&mut p, -400.0));
        assert_eq!(p, -40.0);
    }

    /// MUTATION: adding a wrap or a clamp to `set_y_rot`. Vanilla stores 720.
    #[test]
    fn set_y_rot_neither_wraps_nor_clamps() {
        let mut y = 0.0f32;
        assert!(set_y_rot(&mut y, 720.0));
        assert_eq!(y, 720.0);
        let mut y = 0.0f32;
        assert!(set_y_rot(&mut y, -1000.0));
        assert_eq!(y, -1000.0);
    }

    /// MUTATION: replacing the `is_finite` guards with a clamp or a `0.0`
    /// default. Vanilla logs and returns, leaving the old value standing.
    #[test]
    fn non_finite_rotations_are_discarded() {
        let mut y = 42.0f32;
        assert!(!set_y_rot(&mut y, f32::NAN));
        assert_eq!(y, 42.0);
        assert!(!set_y_rot(&mut y, f32::INFINITY));
        assert_eq!(y, 42.0);
        let mut p = -12.0f32;
        assert!(!set_x_rot(&mut p, f32::NAN));
        assert_eq!(p, -12.0);
        assert!(!set_x_rot(&mut p, f32::NEG_INFINITY));
        assert_eq!(p, -12.0);
    }

    // ── `player_rotation` semantics ─────────────────────────────────────

    /// MUTATION: making either flag compose against a stored "last sent"
    /// rotation instead of the live one, or swapping which flag drives which
    /// axis. The two axes use different flags *and* different values here, so
    /// a swap is visible in both fields.
    #[test]
    fn relative_composes_with_the_current_value_per_axis() {
        let (mut yaw, mut pitch) = (100.0f32, 20.0f32);
        // Relative yaw, absolute pitch.
        apply_relative_rotation(&mut yaw, &mut pitch, 30.0, true, -5.0, false);
        assert_eq!(yaw, 130.0);
        assert_eq!(pitch, -5.0);
        // Absolute yaw, relative pitch.
        apply_relative_rotation(&mut yaw, &mut pitch, 30.0, false, -5.0, true);
        assert_eq!(yaw, 30.0);
        assert_eq!(pitch, -10.0);
    }

    /// The pitch clamp fires *after* the relative add, so a relative step that
    /// would overshoot saturates rather than wrapping around to the far pole.
    ///
    /// MUTATION: clamping `x_rot` **before** the add —
    /// `offset + clamp(x_rot)` instead of `clamp(offset + x_rot)`.
    ///
    /// The first three samples below do **not** catch it, and that is worth
    /// keeping written down: with a base of 80 and a step of +30, clamp-first
    /// gives 110 and [`set_x_rot`]'s own clamp lands it back on 90 — the same
    /// answer. Any step whose magnitude is under 90 leaves `clamp(x_rot)` an
    /// identity, so the mutation is *invisible on every small step*. The
    /// mutation battery caught exactly this: the witness straddled the bound
    /// without ever sitting where the mutation bites.
    ///
    /// The fourth sample is the one that separates them. Base −80, step +400:
    /// clamp-after is `clamp(320) = 90`; clamp-first is `−80 + 90 = 10`. It
    /// needs a base of the *opposite sign* to the step as well as an
    /// out-of-range step — with a positive base both orders saturate to 90.
    #[test]
    fn pitch_clamps_after_the_relative_add() {
        let (mut yaw, mut pitch) = (0.0f32, 80.0f32);
        apply_relative_rotation(&mut yaw, &mut pitch, 0.0, false, 30.0, true);
        assert_eq!(pitch, 90.0);

        let (mut yaw, mut pitch) = (0.0f32, -80.0f32);
        apply_relative_rotation(&mut yaw, &mut pitch, 0.0, false, -30.0, true);
        assert_eq!(pitch, -90.0);

        // Absolute, past a full turn: `calculateAbsolute` clamps to 90 and
        // `setXRot`'s modulo then has nothing left to do.
        let (mut yaw, mut pitch) = (0.0f32, 0.0f32);
        apply_relative_rotation(&mut yaw, &mut pitch, 0.0, false, 400.0, false);
        assert_eq!(pitch, 90.0, "the ±90 clamp precedes setXRot's % 360");

        // The order witness proper.
        let (mut yaw, mut pitch) = (0.0f32, -80.0f32);
        apply_relative_rotation(&mut yaw, &mut pitch, 0.0, false, 400.0, true);
        assert_eq!(
            pitch, 90.0,
            "clamp(-80 + 400) = 90; clamping the step first would give -80 + 90 = 10"
        );
        let _ = yaw;
    }

    /// MUTATION: clamping the yaw, or wrapping it. A yaw teleport past a full
    /// turn is stored raw and is reported to the server raw.
    #[test]
    fn yaw_is_not_bounded_by_the_rotation_packet() {
        let (mut yaw, mut pitch) = (350.0f32, 0.0f32);
        apply_relative_rotation(&mut yaw, &mut pitch, 30.0, true, 0.0, true);
        assert_eq!(yaw, 380.0);
    }

    /// MUTATION: dropping [`clamp_f32`]'s NaN arm, or the setters' guards. A
    /// NaN pitch must leave the previous pitch standing, not write -90 or 90.
    #[test]
    fn nan_pitch_is_discarded_not_clamped() {
        let (mut yaw, mut pitch) = (10.0f32, 25.0f32);
        let (wrote_yaw, wrote_pitch) =
            apply_relative_rotation(&mut yaw, &mut pitch, 5.0, true, f32::NAN, false);
        assert!(wrote_yaw);
        assert!(!wrote_pitch);
        assert_eq!(yaw, 15.0, "the yaw half still applies");
        assert_eq!(pitch, 25.0, "the pitch is unchanged, not clamped");
    }

    // ── `player_look_at` semantics ──────────────────────────────────────

    /// Looking due south (+Z) is yaw 0; due west (-X) is yaw 90. The `- 90.0F`
    /// and the `(zd, xd)` argument order are jointly what produce that.
    ///
    /// MUTATION: swapping `atan2(zd, xd)` for `atan2(xd, zd)`, or dropping the
    /// `- 90.0F`. Each of the four cardinals moves.
    #[test]
    fn look_at_uses_minecraft_yaw_convention() {
        let near = |a: f32, b: f32| (a - b).abs() < 1.0e-3;
        let o = [0.0, 0.0, 0.0];
        let (yaw, pitch) = look_at(o, [0.0, 0.0, 1.0]).unwrap();
        assert!(near(yaw, 0.0), "south is yaw 0, got {yaw}");
        assert!(near(pitch, 0.0));
        let (yaw, _) = look_at(o, [-1.0, 0.0, 0.0]).unwrap();
        assert!(near(yaw, 90.0), "west is yaw 90, got {yaw}");
        let (yaw, _) = look_at(o, [0.0, 0.0, -1.0]).unwrap();
        assert!(near(yaw, -180.0), "north is yaw ±180, got {yaw}");
        let (yaw, _) = look_at(o, [1.0, 0.0, 0.0]).unwrap();
        assert!(near(yaw, -90.0), "east is yaw -90, got {yaw}");
    }

    /// Pitch is negative looking **up**, because of the leading unary minus.
    ///
    /// MUTATION: dropping the `-` in `-(atan2(yd, sd) …)`. Both signs flip and
    /// a straight-ahead sample would not notice.
    #[test]
    fn look_at_pitch_is_negative_upward() {
        let near = |a: f32, b: f32| (a - b).abs() < 1.0e-3;
        let (_, pitch) = look_at([0.0, 0.0, 0.0], [0.0, 1.0, 1.0]).unwrap();
        assert!(near(pitch, -45.0), "up is negative pitch, got {pitch}");
        let (_, pitch) = look_at([0.0, 0.0, 0.0], [0.0, -1.0, 1.0]).unwrap();
        assert!(near(pitch, 45.0), "down is positive pitch, got {pitch}");
        // Straight down: `sd` is 0, `atan2(-1, 0)` is -π/2 → +90.
        let (_, pitch) = look_at([0.0, 0.0, 0.0], [0.0, -1.0, 0.0]).unwrap();
        assert!(near(pitch, 90.0), "straight down is +90, got {pitch}");
    }

    /// The degenerate case vanilla does not special-case: aiming at exactly
    /// your own anchor. `atan2(0, 0)` is 0, so the yaw becomes `0 - 90` and the
    /// pitch 0 — a real, reachable answer rather than a NaN.
    ///
    /// MUTATION: adding a zero-length short circuit that returns `None` or
    /// leaves the rotation alone.
    #[test]
    fn look_at_own_anchor_yields_yaw_minus_90() {
        let (yaw, pitch) = look_at([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]).unwrap();
        assert_eq!(yaw, -90.0);
        assert_eq!(pitch, 0.0);
    }

    /// The anchor is applied to the *viewer*, so the same target seen from the
    /// feet and from the eyes gives different pitches.
    ///
    /// MUTATION: ignoring `from_anchor` (using the player's feet always). The
    /// eye sample would then read -45 like the feet one.
    #[test]
    fn from_anchor_changes_the_pitch() {
        let feet = [0.0, 0.0, 0.0];
        let target = [0.0, 10.0, 10.0];
        let (_, feet_pitch) = look_at(Anchor::Feet.apply(feet, 1.62), target).unwrap();
        let (_, eye_pitch) = look_at(Anchor::Eyes.apply(feet, 1.62), target).unwrap();
        assert!((feet_pitch - -45.0).abs() < 1.0e-3);
        assert!(
            eye_pitch > feet_pitch,
            "from the eyes the target is less far above, so the pitch is \
             less negative: feet {feet_pitch}, eyes {eye_pitch}"
        );
    }

    /// MUTATION: reading `Anchor::from_ordinal` through a `ByIdMap`-style
    /// default (returning `Feet` for anything out of range). `readEnum` is an
    /// array index and 2 is an error.
    #[test]
    fn anchor_ordinals_are_readenum_not_a_default() {
        assert_eq!(Anchor::from_ordinal(0), Some(Anchor::Feet));
        assert_eq!(Anchor::from_ordinal(1), Some(Anchor::Eyes));
        assert_eq!(Anchor::from_ordinal(2), None);
        assert_eq!(Anchor::from_ordinal(-1), None);
    }

    /// A non-finite target declines the write rather than reaching the table.
    ///
    /// MUTATION: removing the finiteness guard. The index saturates, the angle
    /// is nonsense, and `apply_look_at` reports success.
    #[test]
    fn non_finite_target_declines() {
        assert!(look_at([0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0]).is_none());
        assert!(look_at([0.0, 0.0, 0.0], [f64::INFINITY, 0.0, 0.0]).is_none());
        let (mut yaw, mut pitch) = (7.0f32, 8.0f32);
        assert!(!apply_look_at(
            &mut yaw,
            &mut pitch,
            [0.0, 0.0, 0.0],
            [0.0, f64::NAN, 0.0]
        ));
        assert_eq!((yaw, pitch), (7.0, 8.0));
    }
}
