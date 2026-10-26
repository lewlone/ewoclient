//! The local player's attack-strength clock (`Player.attackStrengthTicker`),
//! which the HUD's attack indicator reads.
//!
//! ```java
//! // Player.tick
//! this.attackStrengthTicker++;
//! if (!ItemStack.matches(this.lastItemInMainHand, mainHand)) {
//!    if (!ItemStack.isSameItem(this.lastItemInMainHand, mainHand)) this.resetAttackStrengthTicker();
//!    this.lastItemInMainHand = mainHand.copy();
//! }
//! // Player
//! getCurrentItemAttackStrengthDelay() = (float)(1.0 / getAttributeValue(ATTACK_SPEED) * 20.0)
//! getAttackStrengthScale(a) = clamp((attackStrengthTicker + a) / delay, 0, 1)
//! ```
//!
//! The ticker also resets on `MultiPlayerGameMode.attack`, on `Minecraft
//! .startAttack`'s MISS arm (no target, or a targeted block that is air) and
//! on `stopDestroyBlock`; the caller invokes [`AttackStrength::reset`] there.

/// The ticker and the main-hand item it last saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttackStrength {
    ticker: i32,
    /// `lastItemInMainHand`'s item (`None` is the empty stack).
    last_item: Option<i32>,
}

impl AttackStrength {
    /// One client tick of `Player.tick`'s clock. `main_hand` is the held
    /// stack's item id; a different item (empty counts as one) resets, a
    /// changed count or components do not (`isSameItem`).
    pub fn tick(&mut self, main_hand: Option<i32>) {
        self.ticker = self.ticker.saturating_add(1);
        if self.last_item != main_hand {
            self.reset();
            self.last_item = main_hand;
        }
    }

    /// `resetAttackStrengthTicker()`.
    pub fn reset(&mut self) {
        self.ticker = 0;
    }

    /// `getAttackStrengthScale(a)` for a [`delay`] in ticks.
    pub fn scale(&self, a: f32, delay: f32) -> f32 {
        ((self.ticker as f32 + a) / delay).clamp(0.0, 1.0)
    }
}

/// `Player.getCurrentItemAttackStrengthDelay`: ticks to a full swing at an
/// `attack_speed` attribute value (4.0 for a bare hand).
pub fn delay(attack_speed: f64) -> f32 {
    (1.0 / attack_speed * 20.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare hand (speed 4) is a full swing after 5 ticks; a sword (1.6)
    /// after 12.5. Derived from the formula's definition, `20 / speed`.
    #[test]
    fn the_delay_is_twenty_over_the_speed() {
        assert_eq!(delay(4.0), 5.0);
        assert_eq!(delay(1.6), 12.5);
    }

    /// The scale climbs one tick at a time, clamps at 1, and a changed item
    /// type restarts it from the tick it changed on.
    #[test]
    fn the_scale_climbs_clamps_and_restarts_on_a_new_item() {
        let mut a = AttackStrength::default();
        let d = delay(1.6);
        a.tick(Some(7));
        // The first tick saw a different item than the empty start: reset.
        assert_eq!(a.scale(0.0, d), 0.0);
        for _ in 0..5 {
            a.tick(Some(7));
        }
        assert!((a.scale(0.0, d) - 5.0 / 12.5).abs() < 1e-6);
        assert!((a.scale(0.5, d) - 5.5 / 12.5).abs() < 1e-6);
        for _ in 0..30 {
            a.tick(Some(7));
        }
        assert_eq!(a.scale(0.0, d), 1.0);
        a.tick(Some(8));
        assert_eq!(a.scale(0.0, d), 0.0, "another item type");
        a.tick(None);
        assert_eq!(a.scale(0.0, d), 0.0, "the empty hand is another item too");
    }

    /// An attack or a miss resets the clock without touching the item.
    #[test]
    fn a_reset_restarts_the_count_only() {
        let mut a = AttackStrength::default();
        for _ in 0..20 {
            a.tick(Some(3));
        }
        a.reset();
        assert_eq!(a.scale(0.0, 5.0), 0.0);
        a.tick(Some(3));
        assert!(
            (a.scale(0.0, 5.0) - 0.2).abs() < 1e-6,
            "same item: no reset"
        );
    }
}
