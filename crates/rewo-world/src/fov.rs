//! M233 — the dynamic field of view: `AbstractClientPlayer.getFieldOfViewModifier`,
//! `Camera.tickFov` and `Camera.calculateFov` / `modifyFovBasedOnDeathOrFluid`.
//!
//! Vanilla's FOV is not the option alone. Every client tick the camera eases a
//! modifier halfway towards a target the player computes, and every frame the
//! option is multiplied by that modifier lerped across the partial tick:
//!
//! - flying ×1.1;
//! - the movement-speed attribute (sprint modifier included) over
//!   `Abilities.walkingSpeed`, averaged with 1: sprinting's ×1.3 is a ×1.15
//!   FOV, Speed I's ×1.2 a ×1.1, Slowness narrows it;
//! - drawing a bow narrows it by up to 15% over the first 20 ticks;
//! - looking through a spyglass in first person replaces it all with 0.1.
//!
//! **The camera ticks before the player.** `Minecraft.tick` runs
//! `gameRenderer.tick()` (which ticks the camera) before
//! `level.tickEntities()`, so the target is read from the state the previous
//! tick left. `PlaySession::tick` takes it at the top for the same reason.
//!
//! # Recorded deviations
//!
//! - **The modifier starts at 1.0**, not at the field's Java default 0 (the
//!   clamp makes vanilla's first eased value 0.55: the view opens out from a
//!   zoom over the first few ticks of the first world, mostly behind the
//!   loading screen).
//! - **`fovEffectScale` is 1.0** (the option's default): Rewo has no
//!   accessibility slider for it yet.
//! - **Lava in the camera** does not narrow the FOV: Rewo has no lava table on
//!   the session to ask (water does, through the conduit's water table), and
//!   the water test is the tick's eye, not the frame's interpolated camera.
//! - **The death zoom is not applied to the local player**: the session keeps
//!   no local `deathTime` clock (the death screen comes from the server's
//!   `player_combat_kill`), so [`death_or_fluid`] is passed `None` for it.

/// `Camera.tickFov`'s clamp on the eased modifier.
pub const MODIFIER_MIN: f32 = 0.1;
/// See [`MODIFIER_MIN`].
pub const MODIFIER_MAX: f32 = 1.5;
/// `getFieldOfViewModifier`'s flying factor.
pub const FLYING_FACTOR: f32 = 1.1;
/// The bow's maximum narrowing, reached after [`BOW_FULL_TICKS`].
pub const BOW_NARROWING: f32 = 0.15;
/// `Math.min(getTicksUsingItem() / 20.0F, 1.0F)`.
pub const BOW_FULL_TICKS: f32 = 20.0;
/// What a spyglass in first person returns outright.
pub const SPYGLASS_MODIFIER: f32 = 0.1;
/// `modifyFovBasedOnDeathOrFluid`'s water / lava factor, `0.85714287F` (6/7).
pub const FLUID_FACTOR: f32 = 0.857_142_87;

/// What the camera player is using, as far as the FOV cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsedItem {
    /// Not using anything, or something with no FOV effect.
    #[default]
    Other,
    /// `getUseItem().is(Items.BOW)`, with `getTicksUsingItem()`.
    Bow { ticks: i32 },
    /// `isScoping()`: using a spyglass.
    Spyglass,
}

/// The inputs of `AbstractClientPlayer.getFieldOfViewModifier`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FovInputs {
    /// `getAbilities().flying`.
    pub flying: bool,
    /// `getAbilities().getWalkingSpeed()`.
    pub walking_speed: f32,
    /// `getAttributeValue(Attributes.MOVEMENT_SPEED)`, **with** the sprint
    /// modifier while sprinting.
    pub movement_speed: f64,
    pub using: UsedItem,
    /// `options.getCameraType().isFirstPerson()`.
    pub first_person: bool,
    /// `options.fovEffectScale()`, 0..=1.
    pub effect_scale: f32,
}

/// `AbstractClientPlayer.getFieldOfViewModifier(firstPerson, effectScale)`.
pub fn field_of_view_modifier(i: &FovInputs) -> f32 {
    let mut modifier = 1.0f32;
    if i.flying {
        modifier *= FLYING_FACTOR;
    }
    if i.walking_speed != 0.0 {
        // `(float)this.getAttributeValue(..) / walkingSpeed`: narrowed to a
        // float before the division.
        let speed_factor = i.movement_speed as f32 / i.walking_speed;
        modifier *= (speed_factor + 1.0) / 2.0;
    }
    match i.using {
        UsedItem::Bow { ticks } => {
            let scale = (ticks as f32 / BOW_FULL_TICKS).min(1.0);
            modifier *= 1.0 - scale * scale * BOW_NARROWING;
        }
        // Returned before the effect-scale lerp: a spyglass is never scaled.
        UsedItem::Spyglass if i.first_person => return SPYGLASS_MODIFIER,
        _ => {}
    }
    lerp(i.effect_scale, 1.0, modifier)
}

/// `Mth.lerp(delta, start, end)`.
fn lerp(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

/// `Camera.fovModifier` / `oldFovModifier`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FovModifier {
    old: f32,
    current: f32,
}

impl Default for FovModifier {
    /// 1.0, not vanilla's field default 0 (see the module note).
    fn default() -> Self {
        Self {
            old: 1.0,
            current: 1.0,
        }
    }
}

impl FovModifier {
    /// `Camera.tickFov`: ease halfway to `target`, clamped to [0.1, 1.5].
    pub fn tick(&mut self, target: f32) {
        self.old = self.current;
        self.current =
            (self.current + (target - self.current) * 0.5).clamp(MODIFIER_MIN, MODIFIER_MAX);
    }

    /// `Mth.lerp(partialTicks, oldFovModifier, fovModifier)`.
    pub fn at(&self, partial: f32) -> f32 {
        lerp(partial, self.old, self.current)
    }
}

/// `modifyFovBasedOnDeathOrFluid`: a dying camera entity zooms in over its
/// first 20 death ticks (`deathTime + partialTicks`), and an eye in water or
/// lava narrows the view by the effect-scaled 6/7.
pub fn death_or_fluid(fov: f32, dying: Option<f32>, eye_in_fluid: bool, effect_scale: f32) -> f32 {
    let mut fov = fov;
    if let Some(time) = dying {
        let duration = time.min(20.0);
        fov /= (1.0 - 500.0 / (duration + 500.0)) * 2.0 + 1.0;
    }
    if eye_in_fluid {
        fov *= lerp(effect_scale, 1.0, FLUID_FACTOR);
    }
    fov
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walking() -> FovInputs {
        FovInputs {
            flying: false,
            walking_speed: 0.1,
            movement_speed: 0.1f32 as f64,
            using: UsedItem::Other,
            first_person: true,
            effect_scale: 1.0,
        }
    }

    #[test]
    fn walking_is_one_and_sprinting_is_one_point_one_five() {
        assert_eq!(field_of_view_modifier(&walking()), 1.0);
        let sprint = FovInputs {
            movement_speed: 0.1f32 as f64 * 1.3,
            ..walking()
        };
        assert!((field_of_view_modifier(&sprint) - 1.15).abs() < 1e-6);
        let flying = FovInputs {
            flying: true,
            ..walking()
        };
        assert!((field_of_view_modifier(&flying) - 1.1).abs() < 1e-6);
    }

    /// The bow narrows by `scale² · 0.15`, full after 20 ticks, and it
    /// composes with the speed term rather than replacing it.
    #[test]
    fn a_drawn_bow_narrows_by_fifteen_percent() {
        let at = |ticks| {
            field_of_view_modifier(&FovInputs {
                using: UsedItem::Bow { ticks },
                ..walking()
            })
        };
        assert_eq!(at(0), 1.0);
        assert!((at(10) - (1.0 - 0.25 * 0.15)).abs() < 1e-6);
        assert!((at(20) - 0.85).abs() < 1e-6);
        assert!((at(60) - 0.85).abs() < 1e-6);
        let sprinting_draw = field_of_view_modifier(&FovInputs {
            movement_speed: 0.1f32 as f64 * 1.3,
            using: UsedItem::Bow { ticks: 20 },
            ..walking()
        });
        assert!((sprinting_draw - 1.15 * 0.85).abs() < 1e-6);
    }

    #[test]
    fn a_spyglass_is_a_tenth_in_first_person_only_and_never_scaled() {
        let spy = FovInputs {
            using: UsedItem::Spyglass,
            effect_scale: 0.0,
            ..walking()
        };
        assert_eq!(field_of_view_modifier(&spy), 0.1);
        let third = FovInputs {
            first_person: false,
            effect_scale: 1.0,
            ..spy
        };
        assert_eq!(field_of_view_modifier(&third), 1.0);
    }

    #[test]
    fn effect_scale_zero_turns_every_other_term_off() {
        let i = FovInputs {
            flying: true,
            movement_speed: 0.13,
            using: UsedItem::Bow { ticks: 20 },
            effect_scale: 0.0,
            ..walking()
        };
        assert_eq!(field_of_view_modifier(&i), 1.0);
    }

    /// `tickFov` halves the distance each tick and clamps; the partial-tick
    /// read is a plain lerp between the last two values.
    #[test]
    fn the_modifier_eases_halfway_each_tick() {
        let mut m = FovModifier::default();
        m.tick(0.85);
        assert!((m.at(1.0) - 0.925).abs() < 1e-6);
        assert!((m.at(0.0) - 1.0).abs() < 1e-6);
        assert!((m.at(0.5) - 0.9625).abs() < 1e-6);
        for _ in 0..40 {
            m.tick(0.85);
        }
        assert!((m.at(0.0) - 0.85).abs() < 1e-6);
        m.tick(0.0);
        assert!((m.at(1.0) - 0.425).abs() < 1e-6);
        for _ in 0..10 {
            m.tick(0.0);
        }
        assert_eq!(m.at(1.0), MODIFIER_MIN);
        for _ in 0..40 {
            m.tick(9.0);
        }
        assert_eq!(m.at(1.0), MODIFIER_MAX);
    }

    #[test]
    fn dying_zooms_in_and_water_narrows_by_six_sevenths() {
        assert_eq!(death_or_fluid(70.0, None, false, 1.0), 70.0);
        let dead = death_or_fluid(70.0, Some(40.0), false, 1.0);
        let k = (1.0 - 500.0 / 520.0) * 2.0 + 1.0;
        assert!((dead - 70.0 / k).abs() < 1e-4, "{dead}");
        assert!((death_or_fluid(70.0, None, true, 1.0) - 60.0).abs() < 1e-4);
        assert_eq!(death_or_fluid(70.0, None, true, 0.0), 70.0);
    }
}
