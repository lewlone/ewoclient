//! `AbstractHorse`'s client-side rearing, eating and mouth clocks (M-playtest,
//! `rewo_horse`).
//!
//! The server syncs one byte (`DATA_ID_FLAGS`, index 18) and the client runs
//! the three easing clocks off it every tick; the model reads them lerped
//! (`getStandAnim` / `getEatAnim` / `getMouthAnim`) and the rider's seat reads
//! `standAnimO` (`getPassengerAttachmentPoint`). Until this module the stand
//! clock did not exist, so a horse never reared and its rider never rose with
//! it (the playtest's jump apex sat 0.15 blocks low: that is the seat's
//! `0.15 * standAnimO`).
//!
//! ```java
//! this.standAnimO = this.standAnim;
//! if (this.isStanding()) {
//!    this.eatAnim = 0.0F;
//!    this.eatAnimO = this.eatAnim;
//!    this.standAnim = this.standAnim + ((1.0F - this.standAnim) * 0.4F + 0.05F);
//!    if (this.standAnim > 1.0F) { this.standAnim = 1.0F; }
//! } else {
//!    this.allowStandSliding = false;
//!    this.standAnim = this.standAnim + ((0.8F * this.standAnim * this.standAnim * this.standAnim - this.standAnim) * 0.6F - 0.05F);
//!    if (this.standAnim < 0.0F) { this.standAnim = 0.0F; }
//! }
//! ```
//! (`AbstractHorse.tick`, 26.2 `:600-640`.)
//!
//! Not modelled: `mouthCounter` (only `openMouth()` starts it, and that is a
//! server-side call), `tailCounter` / `sprintCounter` (`moveTail` is a client
//! random roll that drives the tail's yaw swish, which Rewo does not draw).

/// `AbstractHorse.FLAG_EATING`.
pub const FLAG_EATING: u8 = 16;
/// `AbstractHorse.FLAG_STANDING`.
pub const FLAG_STANDING: u8 = 32;
/// `AbstractHorse.FLAG_OPEN_MOUTH`.
pub const FLAG_OPEN_MOUTH: u8 = 64;

/// How many ticks `standIfPossible` keeps a horse up (`setStanding(20)`).
pub const STAND_TICKS: i32 = 20;

/// The per-horse clocks. `Copy`, and all zero for anything that is not a horse
/// (the quiescent early-out in [`Self::tick`] makes carrying it free).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EquineAnim {
    /// `DATA_ID_FLAGS`, as the server last said (or the client wrote).
    flags: u8,
    /// `standCounter`.
    stand_counter: i32,
    /// `allowStandSliding`.
    allow_stand_sliding: bool,
    stand: f32,
    stand_o: f32,
    eat: f32,
    eat_o: f32,
    mouth: f32,
    mouth_o: f32,
}

impl EquineAnim {
    /// A `DATA_ID_FLAGS` entry arrived (`assignValues`: the byte is taken
    /// whole, so a client-side `setFlag` is overwritten).
    pub fn set_flags(&mut self, flags: u8) {
        self.flags = flags;
    }

    pub fn flags(&self) -> u8 {
        self.flags
    }

    /// `isStanding()`.
    pub fn is_standing(&self) -> bool {
        self.flags & FLAG_STANDING != 0
    }

    /// `isEating()`.
    pub fn is_eating(&self) -> bool {
        self.flags & FLAG_EATING != 0
    }

    /// `allowStandSliding`: set by a jump release, cleared the first tick the
    /// horse is not standing. `getRiddenInput` reads it.
    pub fn allow_stand_sliding(&self) -> bool {
        self.allow_stand_sliding
    }

    /// `setStanding(ticks)`: `setEating(false)`, flag 32 on, `standCounter`.
    pub fn set_standing(&mut self, ticks: i32) {
        self.flags &= !FLAG_EATING;
        self.flags |= FLAG_STANDING;
        self.stand_counter = ticks;
    }

    /// `clearStanding()`.
    pub fn clear_standing(&mut self) {
        self.flags &= !FLAG_STANDING;
        self.stand_counter = 0;
    }

    /// `AbstractHorse.onPlayerJump(jumpAmount)` for the part that touches the
    /// rearing: a non-negative release sets `allowStandSliding` and
    /// `standIfPossible()`. `can_rear` is `canPerformRearing()` (false for a
    /// camel or a llama) and the caller has established `isEffectiveAi()`
    /// (the local player steers this horse). The caller checks `isSaddled()`.
    pub fn on_player_jump(&mut self, jump_amount: i32, can_rear: bool) {
        if jump_amount >= 0 {
            self.allow_stand_sliding = true;
            if can_rear {
                self.set_standing(STAND_TICKS);
            }
        }
    }

    /// `AbstractHorse.tick`'s counters and easing clocks, one client tick.
    pub fn tick(&mut self) {
        if self.flags == 0
            && self.stand_counter == 0
            && self.stand == 0.0
            && self.eat == 0.0
            && self.mouth == 0.0
        {
            // At rest: nothing moves, and the previous values equal the current.
            self.stand_o = 0.0;
            self.eat_o = 0.0;
            self.mouth_o = 0.0;
            self.allow_stand_sliding = false;
            return;
        }
        if self.stand_counter > 0 {
            self.stand_counter -= 1;
            if self.stand_counter <= 0 {
                self.clear_standing();
            }
        }
        self.eat_o = self.eat;
        if self.is_eating() {
            self.eat += (1.0 - self.eat) * 0.4 + 0.05;
            if self.eat > 1.0 {
                self.eat = 1.0;
            }
        } else {
            self.eat += (0.0 - self.eat) * 0.4 - 0.05;
            if self.eat < 0.0 {
                self.eat = 0.0;
            }
        }
        self.stand_o = self.stand;
        if self.is_standing() {
            self.eat = 0.0;
            self.eat_o = self.eat;
            self.stand += (1.0 - self.stand) * 0.4 + 0.05;
            if self.stand > 1.0 {
                self.stand = 1.0;
            }
        } else {
            self.allow_stand_sliding = false;
            self.stand += (0.8 * self.stand * self.stand * self.stand - self.stand) * 0.6 - 0.05;
            if self.stand < 0.0 {
                self.stand = 0.0;
            }
        }
        self.mouth_o = self.mouth;
        if self.flags & FLAG_OPEN_MOUTH != 0 {
            self.mouth += (1.0 - self.mouth) * 0.7 + 0.05;
            if self.mouth > 1.0 {
                self.mouth = 1.0;
            }
        } else {
            self.mouth += (0.0 - self.mouth) * 0.7 - 0.05;
            if self.mouth < 0.0 {
                self.mouth = 0.0;
            }
        }
    }

    /// `standAnimO` — what the rider's seat reads.
    pub fn stand_anim_o(&self) -> f32 {
        self.stand_o
    }

    /// `getStandAnim(a)`.
    pub fn stand_anim(&self, a: f32) -> f32 {
        lerp(a, self.stand_o, self.stand)
    }

    /// `getEatAnim(a)`.
    pub fn eat_anim(&self, a: f32) -> f32 {
        lerp(a, self.eat_o, self.eat)
    }

    /// `getMouthAnim(a)`.
    pub fn mouth_anim(&self, a: f32) -> f32 {
        lerp(a, self.mouth_o, self.mouth)
    }
}

/// `Mth.lerp(a, from, to)`: `from + a * (to - from)`.
fn lerp(a: f32, from: f32, to: f32) -> f32 {
    from + a * (to - from)
}

/// `canPerformRearing()`: true for the horse family, false for `Camel`
/// (and the husk) and `Llama` (and the trader's).
pub fn can_perform_rearing(is_camel: bool, type_name: &str) -> bool {
    !is_camel && !matches!(type_name, "minecraft:llama" | "minecraft:trader_llama")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stand clock's rise, from `AbstractHorse.tick`'s literals alone:
    /// each tick `s += (1 - s) * 0.4 + 0.05`, capped at 1. Starting from 0
    /// that is 0.45, then 0.45 + (0.55 * 0.4 + 0.05) = 0.72.
    #[test]
    fn the_stand_clock_rises_by_the_decompiles_recurrence() {
        let mut a = EquineAnim::default();
        a.set_standing(STAND_TICKS);
        a.tick();
        assert_eq!(a.stand_anim_o(), 0.0);
        assert!((a.stand_anim(1.0) - 0.45).abs() < 1e-6);
        a.tick();
        assert!((a.stand_anim(1.0) - 0.72).abs() < 1e-6);
        assert!((a.stand_anim_o() - 0.45).abs() < 1e-7);
        for _ in 0..20 {
            a.tick();
        }
        // The counter ran out after 20 ticks: the flag is gone and the clock
        // fell back below 1.
        assert!(!a.is_standing());
        assert!(a.stand_anim(1.0) < 1.0);
    }

    /// `standCounter` clears the flag on the 20th tick after `setStanding(20)`
    /// (`--standCounter <= 0`), and the fall is
    /// `s += (0.8 s^3 - s) * 0.6 - 0.05`, floored at 0.
    #[test]
    fn the_flag_clears_after_twenty_ticks_and_the_clock_falls_to_zero() {
        let mut a = EquineAnim::default();
        a.set_standing(STAND_TICKS);
        for t in 1..=19 {
            a.tick();
            assert!(a.is_standing(), "tick {t}");
        }
        a.tick();
        assert!(!a.is_standing());
        // At the clamp the clock is exactly 1.0 (it has been rising for 20
        // ticks), so the first falling tick is (0.8 - 1) * 0.6 - 0.05.
        assert_eq!(a.stand_anim_o(), 1.0);
        // (0.8 - 1) * 0.6 - 0.05 = -0.17.
        assert!((a.stand_anim(1.0) - 0.83).abs() < 1e-6);
        for _ in 0..40 {
            a.tick();
        }
        assert_eq!(a.stand_anim(1.0), 0.0);
        assert_eq!(a.stand_anim_o(), 0.0);
    }

    /// `allowStandSliding` is set by a release and cleared by the first tick
    /// the horse is not standing (`else { allowStandSliding = false }`).
    #[test]
    fn a_jump_release_sets_sliding_and_the_first_resting_tick_clears_it() {
        let mut a = EquineAnim::default();
        a.on_player_jump(100, true);
        assert!(a.allow_stand_sliding() && a.is_standing());
        a.tick();
        assert!(a.allow_stand_sliding());
        for _ in 0..25 {
            a.tick();
        }
        assert!(!a.allow_stand_sliding());
        // A negative release neither slides nor rears.
        let mut b = EquineAnim::default();
        b.on_player_jump(-1, true);
        assert!(!b.allow_stand_sliding() && !b.is_standing());
        // A camel slides (the flag is set before `standIfPossible`) but does
        // not rear: `canPerformRearing()` is false.
        let mut c = EquineAnim::default();
        c.on_player_jump(50, false);
        assert!(c.allow_stand_sliding() && !c.is_standing());
    }

    /// `setStanding` first does `setEating(false)`, and a standing horse
    /// zeroes the eat clock (`eatAnim = 0; eatAnimO = eatAnim`).
    #[test]
    fn standing_ends_eating() {
        let mut a = EquineAnim::default();
        a.set_flags(FLAG_EATING);
        for _ in 0..5 {
            a.tick();
        }
        assert!(a.eat_anim(1.0) > 0.9);
        a.set_standing(STAND_TICKS);
        assert!(!a.is_eating());
        a.tick();
        assert_eq!((a.eat_anim(0.0), a.eat_anim(1.0)), (0.0, 0.0));
    }

    /// The eat clock rises `(1 - e) * 0.4 + 0.05` a tick, the mouth clock
    /// `(1 - m) * 0.7 + 0.05`, and each falls by its own factor less 0.05.
    /// From 0 the eat clock reads 0.45, 0.72 and the mouth clock 0.75, 0.975.
    #[test]
    fn the_eat_and_mouth_clocks_follow_their_literals() {
        let mut a = EquineAnim::default();
        a.set_flags(FLAG_EATING | FLAG_OPEN_MOUTH);
        a.tick();
        assert!((a.eat_anim(1.0) - 0.45).abs() < 1e-6);
        assert!((a.mouth_anim(1.0) - 0.75).abs() < 1e-6);
        a.tick();
        assert!((a.eat_anim(1.0) - 0.72).abs() < 1e-6);
        assert!((a.mouth_anim(1.0) - 0.975).abs() < 1e-6);
        a.set_flags(0);
        a.tick();
        // e = 0.72 - 0.72 * 0.4 - 0.05; m = 0.975 - 0.975 * 0.7 - 0.05.
        assert!((a.eat_anim(1.0) - 0.382).abs() < 1e-6);
        assert!((a.mouth_anim(1.0) - 0.2425).abs() < 1e-6);
    }

    #[test]
    fn llamas_and_camels_do_not_rear() {
        assert!(can_perform_rearing(false, "minecraft:horse"));
        assert!(can_perform_rearing(false, "minecraft:donkey"));
        assert!(!can_perform_rearing(true, "minecraft:camel"));
        assert!(!can_perform_rearing(false, "minecraft:llama"));
        assert!(!can_perform_rearing(false, "minecraft:trader_llama"));
    }

    /// A horse that never had a flag set costs one early-out per tick and
    /// stays exactly zero.
    #[test]
    fn a_resting_horse_stays_at_zero() {
        let mut a = EquineAnim::default();
        for _ in 0..10 {
            a.tick();
        }
        assert_eq!(a, EquineAnim::default());
    }
}
