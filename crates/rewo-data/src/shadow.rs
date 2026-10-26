//! `EntityRenderer.getShadowRadius` / `getShadowStrength` for one entity.
//!
//! The base numbers per entity type are the generated
//! [`crate::shadow_radius_table`] (the renderer constructors' `shadow`
//! argument, or `this.shadowRadius = ..`); this module applies the rules that
//! run on top at render time, from the render state Rewo holds elsewhere:
//!
//! ```text
//! EntityRenderer.getShadowRadius        = shadowRadius
//! LivingEntityRenderer.getShadowRadius  = super * state.scale
//! MobRenderer.getShadowRadius           = super * state.ageScale
//! VillagerRenderer / StriderRenderer    = super, baby: * 0.5F
//! TurtleRenderer                        = super, baby: * 0.83F
//! AbstractCubeMobRenderer               = state.size * 0.25F      (no super)
//! PufferfishRenderer                    = 0.1F + 0.1F * puffState (no super)
//! DisplayRenderer                       = the display's own data  (not modelled: 0)
//! ```
//!
//! and `EntityRenderer.extractShadow` caps the result at 32.

pub use crate::shadow_radius_table::{ShadowDef, ShadowRule, SHADOWS};

/// `EntityRenderer.extractShadow`'s cap.
pub const MAX_SHADOW_RADIUS: f32 = 32.0;

/// The table row for an entity type name (with or without `minecraft:`).
pub fn def(entity_name: &str) -> Option<&'static ShadowDef> {
    let short = entity_name
        .strip_prefix("minecraft:")
        .unwrap_or(entity_name);
    SHADOWS
        .binary_search_by_key(&short, |d| d.entity)
        .ok()
        .map(|i| &SHADOWS[i])
}

/// The render state the radius rules read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowInputs {
    /// `state.isBaby` (`LivingEntity.getAgeScale()` is 0.5 for a baby, else 1).
    pub baby: bool,
    /// `state.scale`: the `minecraft:scale` attribute, 1 by default.
    pub scale: f32,
    /// A cube mob's `state.size`.
    pub cube_size: i32,
    /// A pufferfish's `state.puffState`.
    pub puff_state: i32,
}

impl Default for ShadowInputs {
    fn default() -> Self {
        Self {
            baby: false,
            scale: 1.0,
            cube_size: 1,
            puff_state: 0,
        }
    }
}

impl ShadowDef {
    /// `getShadowRadius(state)`, capped at [`MAX_SHADOW_RADIUS`] as
    /// `extractShadow` does.
    pub fn shadow_radius(&self, s: &ShadowInputs) -> f32 {
        let r = match self.rule {
            ShadowRule::CubeSize => s.cube_size as f32 * 0.25,
            ShadowRule::Pufferfish => 0.1 + 0.1 * s.puff_state as f32,
            ShadowRule::Display => 0.0,
            ShadowRule::None | ShadowRule::BabyHalf | ShadowRule::BabyTurtle => {
                let mut r = self.radius;
                if self.living {
                    r *= s.scale;
                }
                if self.mob {
                    r *= if s.baby { 0.5 } else { 1.0 };
                }
                match self.rule {
                    ShadowRule::BabyHalf if s.baby => r * 0.5,
                    ShadowRule::BabyTurtle if s.baby => r * 0.83,
                    _ => r,
                }
            }
        };
        r.min(MAX_SHADOW_RADIUS)
    }

    /// `getShadowStrength(state)`: the table's value (no renderer varies it
    /// except the displays, which are not modelled).
    pub fn shadow_strength(&self) -> f32 {
        match self.rule {
            ShadowRule::Display => 0.0,
            _ => self.strength,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radius(name: &str, s: ShadowInputs) -> f32 {
        def(name)
            .unwrap_or_else(|| panic!("no row for {name}"))
            .shadow_radius(&s)
    }

    /// The table is sorted by entity name (the lookup bisects it) and has one
    /// row per name.
    #[test]
    fn the_table_is_sorted_and_unique() {
        assert!(SHADOWS.windows(2).all(|w| w[0].entity < w[1].entity));
        assert!(def("minecraft:cow").is_some() && def("cow").is_some());
        assert!(def("minecraft:no_such_thing").is_none());
    }

    /// The playtest's two: a cow is 0.7 and a player 0.5; a baby cow takes
    /// `MobRenderer`'s age scale, a baby player (a player has none) does not.
    #[test]
    fn a_baby_mob_is_halved_and_a_baby_player_is_not() {
        let adult = ShadowInputs::default();
        let baby = ShadowInputs {
            baby: true,
            ..adult
        };
        assert_eq!(radius("cow", adult), 0.7);
        assert_eq!(radius("cow", baby), 0.7 * 0.5);
        assert_eq!(radius("player", adult), 0.5);
        assert_eq!(radius("player", baby), 0.5);
    }

    /// `LivingEntityRenderer` multiplies by the scale attribute; a boat (not
    /// living) ignores it.
    #[test]
    fn the_scale_attribute_scales_a_living_shadow_only() {
        let big = ShadowInputs {
            scale: 2.0,
            ..Default::default()
        };
        assert_eq!(radius("pig", big), 1.4);
        assert_eq!(radius("oak_boat", big), 0.8);
    }

    /// The renderers that add their own baby factor on top of `ageScale`
    /// (villager, strider: 0.5 again; turtle: 0.83), and the two that replace
    /// the whole formula.
    #[test]
    fn the_overriding_renderers_follow_their_own_formulas() {
        let baby = ShadowInputs {
            baby: true,
            ..Default::default()
        };
        assert_eq!(radius("villager", baby), 0.5 * 0.5 * 0.5);
        assert_eq!(radius("strider", baby), 0.5 * 0.5 * 0.5);
        assert_eq!(radius("turtle", baby), 0.7 * 0.5 * 0.83);
        assert_eq!(radius("turtle", ShadowInputs::default()), 0.7);
        for (size, want) in [(1, 0.25), (2, 0.5), (4, 1.0)] {
            let s = ShadowInputs {
                cube_size: size,
                ..Default::default()
            };
            assert_eq!(radius("slime", s), want);
            assert_eq!(radius("magma_cube", s), want);
        }
        for (puff, want) in [(0, 0.1), (1, 0.2), (2, 0.3)] {
            let s = ShadowInputs {
                puff_state: puff,
                ..Default::default()
            };
            assert!((radius("pufferfish", s) - want).abs() < 1e-6, "puff {puff}");
        }
    }

    /// The items' and orbs' 0.75 strength, everyone else's 1, a display's 0.
    #[test]
    fn the_strength_comes_from_the_renderer() {
        assert_eq!(def("item").unwrap().shadow_strength(), 0.75);
        assert_eq!(def("experience_orb").unwrap().shadow_strength(), 0.75);
        assert_eq!(def("cow").unwrap().shadow_strength(), 1.0);
        assert_eq!(def("text_display").unwrap().shadow_strength(), 0.0);
        assert_eq!(radius("text_display", ShadowInputs::default()), 0.0);
    }

    /// The radius is capped at `extractShadow`'s 32.
    #[test]
    fn the_radius_is_capped() {
        let huge = ShadowInputs {
            scale: 100.0,
            ..Default::default()
        };
        assert_eq!(radius("cow", huge), MAX_SHADOW_RADIUS);
    }

    /// Independent of the generator: for a spread of entities the radius is
    /// the float literal the renderer's own source passes (or assigns), read
    /// here straight from the decompile.
    #[test]
    fn the_table_agrees_with_the_decompiled_renderers() {
        let Some(paths) = crate::DataPaths::for_version("26.2") else {
            crate::skip_test!("no config dir");
            return;
        };
        let dir = paths
            .root
            .join("decompiled/net/minecraft/client/renderer/entity");
        if !dir.is_dir() {
            crate::skip_test!("no decompile at {}", dir.display());
            return;
        }
        // (entity, renderer file, the literal in its constructor or field).
        for (entity, file, literal) in [
            ("cow", "CowRenderer.java", "0.7F"),
            ("pig", "PigRenderer.java", "0.7F"),
            ("chicken", "ChickenRenderer.java", "0.3F"),
            ("sheep", "SheepRenderer.java", "0.7F"),
            ("spider", "SpiderRenderer.java", "0.8F"),
            ("ghast", "GhastRenderer.java", "1.5F"),
            ("horse", "AbstractHorseRenderer.java", "0.75F"),
            ("wolf", "WolfRenderer.java", "0.5F"),
            ("bat", "BatRenderer.java", "0.25F"),
            ("item", "ItemEntityRenderer.java", "0.15F"),
            ("tnt", "TntRenderer.java", "0.5F"),
            ("oak_boat", "AbstractBoatRenderer.java", "0.8F"),
            ("player", "player/AvatarRenderer.java", "0.5F"),
        ] {
            let src =
                std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(src.contains(literal), "{file} has no {literal}");
            let want: f32 = literal.trim_end_matches('F').parse().unwrap();
            assert_eq!(def(entity).unwrap().radius, want, "{entity}");
        }
    }
}
