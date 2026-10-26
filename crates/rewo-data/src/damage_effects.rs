//! `DamageEffects`: which hurt sound a player makes for a damage type
//! (`Player.getHurtSound` is `source.type().effects().sound()`).
//!
//! `minecraft:damage_type` is a datapack registry. A server on the vanilla pack
//! sends each entry's name with no data (the client reads it from its own
//! jar), so the vanilla `effects` field is kept here; an entry the server does
//! send data for names its own (`DamageEffects::from_name`).

/// The `DamageEffects` enum, with the `SoundEvents` each plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DamageEffects {
    #[default]
    Hurt,
    Thorns,
    Drowning,
    Burning,
    Poking,
    Freezing,
}

impl DamageEffects {
    /// `getSerializedName`'s inverse.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "hurt" => Self::Hurt,
            "thorns" => Self::Thorns,
            "drowning" => Self::Drowning,
            "burning" => Self::Burning,
            "poking" => Self::Poking,
            "freezing" => Self::Freezing,
            _ => return None,
        })
    }

    /// `DamageEffects.sound()`. In 26.2 THORNS plays the plain hurt sound.
    pub fn sound(self) -> &'static str {
        match self {
            Self::Hurt | Self::Thorns => "minecraft:entity.player.hurt",
            Self::Drowning => "minecraft:entity.player.hurt_drown",
            Self::Burning => "minecraft:entity.player.hurt_on_fire",
            Self::Poking => "minecraft:entity.player.hurt_sweet_berry_bush",
            Self::Freezing => "minecraft:entity.player.hurt_freeze",
        }
    }
}

/// The vanilla pack's damage types whose `effects` is not the default
/// `hurt` (`data/minecraft/damage_type/*.json`; the test checks the jar).
const VANILLA: &[(&str, DamageEffects)] = &[
    ("minecraft:campfire", DamageEffects::Burning),
    ("minecraft:drown", DamageEffects::Drowning),
    ("minecraft:fireball", DamageEffects::Burning),
    ("minecraft:freeze", DamageEffects::Freezing),
    ("minecraft:hot_floor", DamageEffects::Burning),
    ("minecraft:in_fire", DamageEffects::Burning),
    ("minecraft:lava", DamageEffects::Burning),
    ("minecraft:on_fire", DamageEffects::Burning),
    ("minecraft:sulfur_cube_hot", DamageEffects::Burning),
    ("minecraft:sweet_berry_bush", DamageEffects::Poking),
    ("minecraft:thorns", DamageEffects::Thorns),
    ("minecraft:unattributed_fireball", DamageEffects::Burning),
];

/// The vanilla pack's `effects` for a damage type by registry name.
pub fn vanilla(name: &str) -> DamageEffects {
    VANILLA
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(DamageEffects::Hurt, |&(_, e)| e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every damage type file in the decompiled jar's data agrees with
    /// [`vanilla`], including the ones that leave `effects` at its default.
    #[test]
    fn the_table_matches_the_jars_damage_types() {
        let dir = dirs::config_dir()
            .unwrap_or_default()
            .join("EwoClient/rewo/26.2/decompiled/data/minecraft/damage_type");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            crate::skip_test!("no local 26.2 decompile at {}", dir.display());
            return;
        };
        let mut checked = 0;
        for e in entries.flatten() {
            let path = e.path();
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let want = json
                .get("effects")
                .and_then(|v| v.as_str())
                .map_or(DamageEffects::Hurt, |s| {
                    DamageEffects::from_name(s).unwrap()
                });
            assert_eq!(vanilla(&format!("minecraft:{stem}")), want, "{stem}");
            checked += 1;
        }
        assert!(checked > 40, "only {checked} damage types read");
    }

    #[test]
    fn effects_name_their_hurt_sounds() {
        assert_eq!(
            vanilla("minecraft:drown").sound(),
            "minecraft:entity.player.hurt_drown"
        );
        assert_eq!(
            vanilla("minecraft:generic").sound(),
            "minecraft:entity.player.hurt"
        );
        assert_eq!(
            vanilla("minecraft:thorns").sound(),
            "minecraft:entity.player.hurt"
        );
    }
}
