//! `LocalPlayer`'s jump-riding meter (M169) — the input the jump bar draws,
//! and the one serverbound packet it sends.
//!
//! ```java
//! PlayerRideableJumping jumpableVehicle = this.jumpableVehicle();
//! if (jumpableVehicle != null && jumpableVehicle.getJumpCooldown() == 0) {
//!    if (this.jumpRidingTicks < 0) {
//!       this.jumpRidingTicks++;
//!       if (this.jumpRidingTicks == 0) {
//!          this.jumpRidingScale = 0.0F;
//!       }
//!    }
//!
//!    if (wasJumping && !this.input.keyPresses.jump()) {
//!       this.jumpRidingTicks = -10;
//!       jumpableVehicle.onPlayerJump(Mth.floor(this.getJumpRidingScale() * 100.0F));
//!       this.sendRidingJump();
//!    } else if (!wasJumping && this.input.keyPresses.jump()) {
//!       this.jumpRidingTicks = 0;
//!       this.jumpRidingScale = 0.0F;
//!    } else if (wasJumping) {
//!       this.jumpRidingTicks++;
//!       if (this.jumpRidingTicks < 10) {
//!          this.jumpRidingScale = this.jumpRidingTicks * 0.1F;
//!       } else {
//!          this.jumpRidingScale = 0.8F + 2.0F / (this.jumpRidingTicks - 9) * 0.1F;
//!       }
//!    }
//! } else {
//!    this.jumpRidingScale = 0.0F;
//! }
//! ```
//! (`LocalPlayer.java:882-908`, with `wasJumping = this.input.keyPresses.jump()`
//! sampled at `:773` BEFORE `input.tick()` — i.e. the PREVIOUS tick's key
//! state, which is what [`JumpRiding::tick`] keeps as `was_jumping`.)
//!
//! # Things a tidy rewrite gets wrong
//!
//! * **There is no literal 1.0 cap and no 0.1 floor.** The values are the
//!   formula's: tick 1 is 0.1, tick 9 is 0.9, tick 10 is `0.8 + 2/1 * 0.1 =
//!   1.0`, and from there it DECAYS toward 0.8 — tick 11 is 0.9, tick 19 is
//!   0.82. A bar that clamps at full after ten ticks is wrong for every tick
//!   after the tenth.
//! * **The release does not zero the scale.** It parks `jumpRidingTicks` at
//!   `-10`, and the scale only drops to 0 when that counter climbs back to
//!   zero — ten ticks later. The bar stays full through the jump.
//! * **The press zeroes the scale**, not the release — `ticks = 0, scale = 0`
//!   on the rising edge.
//! * **The cooldown branch is the `else`.** While a camel's dash cooldown runs,
//!   NOTHING inside the block happens: no edge detection, no send. A release
//!   during the cooldown is simply lost, and the `-10` park is never entered.
//! * **The packet's data is `Mth.floor(scale * 100)`**, sent on the release
//!   only; `onPlayerJump` on the vehicle is the client's own cosmetic pending
//!   scale and is not modelled here.
//!
//! The `jumpableVehicle()` half — saddled, controlled by this player, not a
//! sitting camel — lives beside the session, because it reads the entity
//! table; this struct takes its answer as [`JumpableVehicle`].

/// `PlayerRideableJumping` as the meter needs it: present iff
/// `LocalPlayer.jumpableVehicle()` would be non-null, with
/// `getJumpCooldown()` — a camel's or nautilus's dash cooldown, a horse's
/// interface default 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JumpableVehicle {
    pub cooldown: i32,
}

/// `ServerboundPlayerCommandPacket.Action.START_RIDING_JUMP` — the enum's
/// fourth constant (`STOP_SLEEPING, START_SPRINTING, STOP_SPRINTING,
/// START_RIDING_JUMP, ...`, `ServerboundPlayerCommandPacket.java:60-68`),
/// written with `writeEnum`, i.e. its ordinal as a VarInt.
pub const START_RIDING_JUMP: i32 = 3;

/// `LocalPlayer.jumpRidingTicks` / `jumpRidingScale`, plus the previous
/// tick's key state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JumpRiding {
    ticks: i32,
    scale: f32,
    was_jumping: bool,
}

impl JumpRiding {
    /// One `aiStep` with this tick's jump key. Returns the `data` of a
    /// `START_RIDING_JUMP` to send — `Mth.floor(scale * 100)` — when the key
    /// was released this tick on a vehicle whose cooldown is 0.
    pub fn tick(&mut self, jumping: bool, vehicle: Option<JumpableVehicle>) -> Option<i32> {
        let was_jumping = self.was_jumping;
        self.was_jumping = jumping;
        let mut send = None;
        match vehicle {
            Some(v) if v.cooldown == 0 => {
                if self.ticks < 0 {
                    self.ticks += 1;
                    if self.ticks == 0 {
                        self.scale = 0.0;
                    }
                }
                if was_jumping && !jumping {
                    self.ticks = -10;
                    send = Some(riding_jump_data(self.scale));
                } else if !was_jumping && jumping {
                    self.ticks = 0;
                    self.scale = 0.0;
                } else if was_jumping {
                    self.ticks += 1;
                    if self.ticks < 10 {
                        self.scale = self.ticks as f32 * 0.1;
                    } else {
                        self.scale = 0.8 + 2.0 / (self.ticks - 9) as f32 * 0.1;
                    }
                }
            }
            _ => self.scale = 0.0,
        }
        send
    }

    /// `getJumpRidingScale()`.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn ticks(&self) -> i32 {
        self.ticks
    }
}

/// `Mth.floor(this.getJumpRidingScale() * 100.0F)` — the packet's `data`.
pub fn riding_jump_data(scale: f32) -> i32 {
    (scale * 100.0).floor() as i32
}

/// A `player_command` body: VarInt entity id, VarInt action ordinal, VarInt
/// data (`ServerboundPlayerCommandPacket.write`, `:33-37`).
pub fn player_command_body(entity_id: i32, action: i32, data: i32) -> Vec<u8> {
    let mut w = rewo_proto::writer::PacketWriter::default();
    w.varint(entity_id);
    w.varint(action);
    w.varint(data);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HORSE: Option<JumpableVehicle> = Some(JumpableVehicle { cooldown: 0 });

    fn hold(j: &mut JumpRiding, ticks: usize) -> Vec<f32> {
        (0..ticks).map(|_| {
            j.tick(true, HORSE);
            j.scale()
        }).collect()
    }

    /// The ramp is the formula's: no 1.0 cap, no 0.1 floor, a decay after ten.
    #[test]
    fn the_ramp_climbs_by_tenths_peaks_at_ten_and_decays_toward_point_eight() {
        let mut j = JumpRiding::default();
        // Tick 0: the press (rising edge) zeroes; ticks 1..=9 are n * 0.1.
        let v = hold(&mut j, 12);
        assert_eq!(v[0], 0.0, "the press resets");
        for (i, s) in v.iter().enumerate().take(10).skip(1) {
            assert!((s - i as f32 * 0.1).abs() < 1e-6, "tick {i}: {s}");
        }
        assert!((v[10] - 1.0).abs() < 1e-6, "tick 10: 0.8 + 2/1 * 0.1");
        assert!((v[11] - 0.9).abs() < 1e-6, "tick 11: 0.8 + 2/2 * 0.1");
        let v = hold(&mut j, 8); // ticks 12..=19 (the first hold ended on tick 11)
        assert!((v[7] - 0.82).abs() < 1e-6, "tick 19: 0.8 + 2/10 * 0.1 = 0.82; got {}", v[7]);
        assert!((v[6] - (0.8 + 2.0 / 9.0 * 0.1)).abs() < 1e-6, "tick 18");
        assert!(v.iter().all(|s| *s > 0.8), "decays TOWARD 0.8, never below");
    }

    /// The release sends `floor(scale * 100)` and parks the counter at -10;
    /// the scale holds until the park counts back to zero.
    #[test]
    fn the_release_sends_and_the_scale_holds_for_ten_ticks() {
        let mut j = JumpRiding::default();
        hold(&mut j, 6); // press + 5 held -> 0.5
        assert!((j.scale() - 0.5).abs() < 1e-6);
        assert_eq!(j.tick(false, HORSE), Some(50), "floor(0.5 * 100)");
        assert_eq!(j.ticks(), -10);
        for _ in 0..9 {
            assert_eq!(j.tick(false, HORSE), None);
            assert!((j.scale() - 0.5).abs() < 1e-6, "the bar stays up through the jump");
        }
        assert_eq!(j.tick(false, HORSE), None);
        assert_eq!((j.ticks(), j.scale()), (0, 0.0), "the tenth tick zeroes it");
        // `floor`, not round: 0.99 -> 99, and 1.0 -> 100.
        assert_eq!(riding_jump_data(0.999), 99);
        assert_eq!(riding_jump_data(1.0), 100);
    }

    /// The rising edge ZEROES the scale, which only shows on a re-press mid
    /// park: after a release the scale holds (`ticks == -10`), and pressing
    /// again before the park counts back to zero must reset it to 0, not
    /// resume the held value.
    #[test]
    fn a_re_press_mid_park_zeroes_the_held_scale() {
        let mut j = JumpRiding::default();
        hold(&mut j, 6); // -> 0.5
        assert_eq!(j.tick(false, HORSE), Some(50));
        assert!((j.scale() - 0.5).abs() < 1e-6, "the park holds the scale");
        assert!(j.ticks() < 0, "still mid-park");
        // Re-press: rising edge -> ticks 0, scale 0. Without the zero it would
        // stay 0.5 and the next held tick would jump to 0.1 off a live 0.5.
        j.tick(true, HORSE);
        assert_eq!((j.ticks(), j.scale()), (0, 0.0), "the press resets a held scale");
        j.tick(true, HORSE);
        assert!((j.scale() - 0.1).abs() < 1e-6, "and the ramp restarts from tick 1");
    }

    /// No vehicle, or a vehicle on cooldown, is the `else`: scale 0 and no
    /// edge detection at all, so a release on cooldown is simply lost.
    #[test]
    fn no_vehicle_or_a_cooldown_is_the_else_branch() {
        let mut j = JumpRiding::default();
        hold(&mut j, 6);
        assert_eq!(j.tick(true, None), None);
        assert_eq!(j.scale(), 0.0, "dismounting zeroes it");
        let mut j = JumpRiding::default();
        hold(&mut j, 6);
        let camel = Some(JumpableVehicle { cooldown: 3 });
        assert_eq!(j.tick(false, camel), None, "the release during a dash cooldown sends nothing");
        assert_eq!(j.scale(), 0.0);
        // And after the cooldown clears, a fresh press starts from zero.
        assert_eq!(j.tick(true, HORSE), None);
        assert_eq!((j.ticks(), j.scale()), (0, 0.0));
    }

    #[test]
    fn the_packet_is_three_varints() {
        assert_eq!(player_command_body(7, START_RIDING_JUMP, 100), vec![7, 3, 100]);
        assert_eq!(START_RIDING_JUMP, 3, "the enum's fourth constant");
    }
}
