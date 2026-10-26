//! The compass and clock needles (`CompassAngle`, `Time`).
//!
//! 26.x draws a compass as one of 32 baked frames (a clock one of 64) chosen
//! by a `range_dispatch` over the needle's value, which depends on where the
//! owner is and which way it faces. The bake keeps every frame as a variant
//! ([`rewo_data::item_models::use_variant_leaves`]); this module computes the
//! value once per frame and hands it to the one place that picks the frame.
//!
//! Recorded approximations: the lodestone target is not drawn (it needs the
//! stack's `lodestone_tracker`, so a lodestone compass points at the world
//! spawn); a needle with no valid target (the recovery compass before a
//! death, any compass outside its target's dimension) rests at turn 0 where
//! vanilla spins it at random; the owner's yaw is the head yaw where vanilla
//! reads the body yaw (`yBodyRot`); a compass held by another entity shows
//! the local player's needle (vanilla evaluates it from that entity), and one
//! lying on the ground or in a frame shows the first frame.

use super::*;
use rewo_data::item_models::{CompassAim, Needles, SelectionContext, Wobbler};

/// The items whose models are needles.
const NEEDLE_ITEMS: [&str; 3] = [
    "minecraft:compass",
    "minecraft:recovery_compass",
    "minecraft:clock",
];

/// What the needle properties read this frame: the values, and whether the
/// owner is in the Overworld (the clock's `context_dimension` select).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NeedleFrame {
    pub needles: Needles,
    pub overworld: bool,
}

impl NeedleFrame {
    const REST: Self = Self {
        needles: Needles {
            compass_spawn: 0.0,
            compass_recovery: 0.0,
            clock_daytime: 0.0,
        },
        overworld: true,
    };
}

/// This frame's needles, set by [`NeedleClock::advance`] and read by every
/// site that picks a needle item's frame (the hotbar, the screens' slots and
/// the first-person and third-person hands). A static for the reason
/// `USE_VARIANTS` is one: the readers are spread over code that does not own
/// the app.
static FRAME: std::sync::Mutex<NeedleFrame> = std::sync::Mutex::new(NeedleFrame::REST);

fn current() -> NeedleFrame {
    FRAME.lock().map_or(NeedleFrame::REST, |f| *f)
}

pub(crate) fn set_frame(f: NeedleFrame) {
    if let Ok(mut slot) = FRAME.lock() {
        *slot = f;
    }
}

/// The selection context the needle items reduce in: `base` (the display
/// context of the draw) with this frame's needles and dimension.
pub(crate) fn with_current_needles(base: SelectionContext<'_>) -> SelectionContext<'_> {
    let f = current();
    base.with_needles(
        Some(if f.overworld {
            "minecraft:overworld"
        } else {
            "minecraft:other"
        }),
        f.needles,
    )
}

/// The model a slot icon of item `name` draws: the needle item's current
/// frame, or `name` for anything else.
pub(crate) fn needle_model(name: &str) -> String {
    if !NEEDLE_ITEMS.contains(&name) {
        return name.to_string();
    }
    let Some(variants) = USE_VARIANTS.get() else {
        return name.to_string();
    };
    variants
        .select(name, with_current_needles(SelectionContext::gui()))
        .to_string()
}

/// The world position a compass looks at, if it is in `dimension`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Target<'a> {
    pub dimension: &'a str,
    pub block: [i32; 3],
}

/// Everything one frame's needles are computed from.
pub(crate) struct NeedleInputs<'a> {
    /// The owner's feet position.
    pub owner: [f64; 3],
    pub yaw_deg: f32,
    /// The level the owner is in.
    pub dimension: Option<&'a str>,
    /// `level.getRespawnData().globalPos()`.
    pub spawn: Option<Target<'a>>,
    /// `player.getLastDeathLocation()`.
    pub death: Option<Target<'a>>,
    /// The world clock, for the Overworld clock.
    pub day_ticks: Option<i64>,
}

/// `CompassAngleState.isValidCompassTargetPos` plus the aim: the target must
/// be in the owner's dimension and not on top of the owner.
pub(crate) fn aim_at(inputs: &NeedleInputs<'_>, target: Option<Target<'_>>) -> Option<CompassAim> {
    let t = target?;
    if inputs.dimension != Some(t.dimension) {
        return None;
    }
    CompassAim::new(inputs.owner, inputs.yaw_deg, t.block)
}

/// The three damped needles, advanced once per game tick like vanilla's
/// per-model wobblers.
pub(crate) struct NeedleClock {
    spawn: Wobbler,
    recovery: Wobbler,
    clock: Wobbler,
}

impl Default for NeedleClock {
    fn default() -> Self {
        Self {
            // `CompassAngleState` pulls with 0.8, `Time` with 0.9.
            spawn: Wobbler::new(0.8),
            recovery: Wobbler::new(0.8),
            clock: Wobbler::new(0.9),
        }
    }
}

impl NeedleClock {
    /// This frame's needles for the local player at game tick `tick`.
    pub(crate) fn advance(&mut self, tick: i64, inputs: &NeedleInputs<'_>) -> NeedleFrame {
        let overworld = inputs.dimension == Some("minecraft:overworld");
        let compass = |wobbler: &mut Wobbler, target: Option<Target<'_>>| -> f32 {
            match aim_at(inputs, target) {
                // The local player's needle is the angle plus the swing of a
                // wobbler pulled toward `0.5 - (ownerYRotation - 0.25)`.
                Some(aim) => {
                    wobbler.update(tick, aim.wobble_target());
                    aim.turns_wobbled(wobbler)
                }
                None => 0.0,
            }
        };
        let compass_spawn = compass(&mut self.spawn, inputs.spawn);
        let compass_recovery = compass(&mut self.recovery, inputs.death);
        // `SUN_ANGLE / 360` through the clock's wobbler. The Overworld sky is
        // the only one Rewo evaluates (`celestial_at`), and the clock reads
        // its dial only there.
        let sun = rewo_world::celestial::celestial_at(inputs.day_ticks.unwrap_or(6000))
            .sun_angle_deg
            / 360.0;
        self.clock.update(tick, sun);
        NeedleFrame {
            needles: Needles {
                compass_spawn,
                compass_recovery,
                clock_daytime: self.clock.rotation(),
            },
            overworld,
        }
    }
}

/// This frame's needle inputs from the session.
pub(crate) fn inputs_of(session: &PlaySession) -> NeedleInputs<'_> {
    let p = &session.player;
    NeedleInputs {
        owner: [p.x, p.y, p.z],
        yaw_deg: p.yaw,
        dimension: session.active_dimension_key.as_deref(),
        spawn: session.client_state.respawn_data().map(|r| Target {
            dimension: r.dimension.as_str(),
            block: [r.pos.0, r.pos.1, r.pos.2],
        }),
        death: session.last_death_location.as_ref().map(|d| Target {
            dimension: d.dimension.as_str(),
            block: [d.x, d.y, d.z],
        }),
        day_ticks: session.day_ticks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(dimension: &'a str, yaw: f32, spawn: Target<'a>, day: i64) -> NeedleInputs<'a> {
        NeedleInputs {
            owner: [0.5, 64.0, 0.5],
            yaw_deg: yaw,
            dimension: Some(dimension),
            spawn: Some(spawn),
            death: None,
            day_ticks: Some(day),
        }
    }

    const SOUTH: Target<'static> = Target {
        dimension: "minecraft:overworld",
        block: [0, 64, 100],
    };

    /// A target in another dimension has no direction, whatever its block.
    #[test]
    fn a_target_in_another_dimension_is_not_aimed_at() {
        let i = inputs("minecraft:the_nether", 0.0, SOUTH, 6000);
        assert!(aim_at(&i, i.spawn).is_none());
        let i = inputs("minecraft:overworld", 0.0, SOUTH, 6000);
        assert!(aim_at(&i, i.spawn).is_some());
        assert!(aim_at(&i, None).is_none());
    }

    /// Standing still facing the spawn, the compass settles pointing ahead
    /// (turn 0); facing west with the spawn to the south leaves it a quarter
    /// turn off.
    #[test]
    fn a_settled_compass_points_at_the_spawn() {
        let settle = |yaw: f32| {
            let mut clock = NeedleClock::default();
            let i = inputs("minecraft:overworld", yaw, SOUTH, 6000);
            let mut f = clock.advance(1, &i);
            for tick in 2..400 {
                f = clock.advance(tick, &i);
            }
            f.needles.compass_spawn
        };
        let ahead = settle(0.0);
        assert!(ahead < 5e-3 || ahead > 1.0 - 5e-3, "facing it: {ahead}");
        let side = settle(90.0);
        assert!(
            (side - 0.25).abs() < 5e-3 || (side - 0.75).abs() < 5e-3,
            "{side}"
        );
    }

    /// With no valid target the needles rest at zero.
    #[test]
    fn an_untargeted_needle_rests_at_zero() {
        let mut clock = NeedleClock::default();
        let nether = inputs("minecraft:the_nether", 33.0, SOUTH, 6000);
        let f = clock.advance(1, &nether);
        assert_eq!(f.needles.compass_spawn, 0.0);
        assert_eq!(f.needles.compass_recovery, 0.0);
        assert!(!f.overworld);
    }

    /// Noon is the clock's zero and midnight its half turn, after the damped
    /// needle has swung round to it.
    #[test]
    fn the_clock_settles_on_the_sun() {
        let mut clock = NeedleClock::default();
        let noon = inputs("minecraft:overworld", 0.0, SOUTH, 6000);
        let mut f = clock.advance(1, &noon);
        assert!(f.needles.clock_daytime.abs() < 1e-4);
        let midnight = inputs("minecraft:overworld", 0.0, SOUTH, 18000);
        for tick in 2..600 {
            f = clock.advance(tick, &midnight);
        }
        assert!(
            (f.needles.clock_daytime - 0.5).abs() < 5e-3,
            "{:?}",
            f.needles
        );
    }

    /// Only the three needle items are routed through the variants.
    #[test]
    fn other_items_keep_their_name() {
        assert_eq!(needle_model("minecraft:bow"), "minecraft:bow");
        assert_eq!(
            needle_model("minecraft:diamond_sword"),
            "minecraft:diamond_sword"
        );
    }
}
