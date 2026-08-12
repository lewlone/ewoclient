//! `Music`, `Musics` and `BackgroundMusic` — which track, and how often (M145).
//!
//! The sibling of [`crate::ambient`]: both are environment attributes a biome
//! or a dimension type declares, both arrive on the wire in the same
//! `attributes` compound, and both are **replaced** rather than merged when a
//! biome declares one. Reading this next to `AmbientSounds` is the fastest way
//! to see the shape.
//!
//! **This module decides nothing about when a track starts.** That is
//! `MusicManager`'s, in `rewo-net`, and it reads the record this produces.

/// `net.minecraft.sounds.Music` — a track and the window between plays.
///
/// ```java
/// public record Music(Holder<SoundEvent> sound, int minDelay, int maxDelay, boolean replaceCurrentMusic)
/// ```
///
/// `minDelay`/`maxDelay` are **ticks between songs**, not a track length —
/// nothing here knows how long the `.ogg` is, and `MusicManager` never asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Music {
    /// The sound event id, e.g. `minecraft:music.game`.
    pub sound: String,
    pub min_delay: i32,
    pub max_delay: i32,
    /// **Whether this track interrupts a different one already playing.** Set
    /// for the menu, the credits, the End and the dragon fight; clear for
    /// ordinary game and creative music, which is why walking into a jungle
    /// does not cut off the track you are already hearing.
    pub replace_current_music: bool,
}

impl Music {
    pub fn new(sound: impl Into<String>, min_delay: i32, max_delay: i32, replace: bool) -> Music {
        Music {
            sound: sound.into(),
            min_delay,
            max_delay,
            replace_current_music: replace,
        }
    }
}

/// `Musics` — the seven hard-coded tracks (`Musics.java:11-17`).
///
/// Functions rather than constants because a `Music` owns its identifier as a
/// `String`; the numbers are the file's, unrounded.
pub mod musics {
    use super::Music;

    /// `createGameMusic` — 10 to 20 minutes apart, and does **not** replace.
    pub fn create_game_music(sound: impl Into<String>) -> Music {
        Music::new(sound, 12_000, 24_000, false)
    }

    /// One to thirty seconds, and replaces — the menu is meant to start at once.
    pub fn menu() -> Music {
        Music::new("minecraft:music.menu", 20, 600, true)
    }
    pub fn creative() -> Music {
        Music::new("minecraft:music.creative", 12_000, 24_000, false)
    }
    /// Zero delay both ways: the credits and the dragon start immediately and
    /// restart immediately.
    pub fn credits() -> Music {
        Music::new("minecraft:music.credits", 0, 0, true)
    }
    pub fn end_boss() -> Music {
        Music::new("minecraft:music.dragon", 0, 0, true)
    }
    pub fn end() -> Music {
        Music::new("minecraft:music.end", 6_000, 24_000, true)
    }
    pub fn under_water() -> Music {
        create_game_music("minecraft:music.under_water")
    }
    pub fn game() -> Music {
        create_game_music("minecraft:music.game")
    }
}

/// `net.minecraft.world.attribute.BackgroundMusic` — the three tracks a place
/// can offer, and the rule that picks between them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackgroundMusic {
    pub default_music: Option<Music>,
    pub creative_music: Option<Music>,
    pub underwater_music: Option<Music>,
}

impl BackgroundMusic {
    /// `BackgroundMusic.EMPTY` — no music at all, which a biome may declare
    /// **explicitly** (`OverworldBiomes.java:596`). Absent and empty are
    /// different: absent inherits, empty silences.
    pub fn empty() -> BackgroundMusic {
        BackgroundMusic::default()
    }

    /// The one-argument constructor: a game track and nothing else.
    pub fn of_sound(sound: impl Into<String>) -> BackgroundMusic {
        BackgroundMusic {
            default_music: Some(musics::create_game_music(sound)),
            ..BackgroundMusic::default()
        }
    }

    /// `BackgroundMusic.OVERWORLD` — game music, and creative music in creative.
    pub fn overworld() -> BackgroundMusic {
        BackgroundMusic {
            default_music: Some(musics::game()),
            creative_music: Some(musics::creative()),
            underwater_music: None,
        }
    }

    pub fn with_underwater(mut self, underwater: Music) -> BackgroundMusic {
        self.underwater_music = Some(underwater);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.default_music.is_none()
            && self.creative_music.is_none()
            && self.underwater_music.is_none()
    }

    /// `BackgroundMusic.select(isCreative, isUnderwater)`.
    ///
    /// ```java
    /// if (isUnderwater && this.underwaterMusic.isPresent()) return this.underwaterMusic;
    /// else return isCreative && this.creativeMusic.isPresent() ? this.creativeMusic : this.defaultMusic;
    /// ```
    ///
    /// Two things read backwards. **Underwater wins over creative**, so a
    /// creative player swimming in an ocean gets the underwater track and not
    /// the creative one. And each arm falls through to `defaultMusic` only when
    /// its own slot is *absent* — a biome that declares creative music but no
    /// default is silent for everyone except a creative player, rather than
    /// falling back to the game track.
    pub fn select(&self, is_creative: bool, is_underwater: bool) -> Option<&Music> {
        if is_underwater && self.underwater_music.is_some() {
            return self.underwater_music.as_ref();
        }
        if is_creative && self.creative_music.is_some() {
            return self.creative_music.as_ref();
        }
        self.default_music.as_ref()
    }

    /// The value at a position: the dimension's base, **replaced** by the
    /// biome's if that biome declares one.
    ///
    /// Total replacement rather than field-wise, for the reason
    /// [`crate::ambient::AmbientSounds::resolve`] sets out at length: the
    /// biome's entry is the bare-value form of an attribute modifier and every
    /// vanilla biome's modifier is a set. Merging field-wise would give every
    /// Nether biome the Overworld's creative track, because the Nether biomes
    /// declare only a default.
    pub fn resolve(dimension_base: &BackgroundMusic, biome: Option<&BackgroundMusic>) -> BackgroundMusic {
        match biome {
            Some(over) => over.clone(),
            None => dimension_base.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_tracks_carry_their_own_windows() {
        // Literal, from `Musics.java:11-17`. A witness that recomputed them from
        // `create_game_music` would agree with any constant.
        assert_eq!(musics::menu(), Music::new("minecraft:music.menu", 20, 600, true));
        assert_eq!(
            musics::creative(),
            Music::new("minecraft:music.creative", 12_000, 24_000, false)
        );
        assert_eq!(
            musics::credits(),
            Music::new("minecraft:music.credits", 0, 0, true)
        );
        assert_eq!(
            musics::end_boss(),
            Music::new("minecraft:music.dragon", 0, 0, true)
        );
        assert_eq!(musics::end(), Music::new("minecraft:music.end", 6_000, 24_000, true));
        assert_eq!(
            musics::game(),
            Music::new("minecraft:music.game", 12_000, 24_000, false)
        );
        assert_eq!(
            musics::under_water(),
            Music::new("minecraft:music.under_water", 12_000, 24_000, false)
        );
    }

    /// **Only four of the seven replace**, and which four is the whole reason
    /// walking between biomes does not cut off the track you are hearing.
    #[test]
    fn game_music_does_not_replace_and_the_rest_do() {
        assert!(!musics::game().replace_current_music);
        assert!(!musics::creative().replace_current_music);
        assert!(!musics::under_water().replace_current_music);
        assert!(musics::menu().replace_current_music);
        assert!(musics::credits().replace_current_music);
        assert!(musics::end().replace_current_music);
        assert!(musics::end_boss().replace_current_music);
    }

    /// **Underwater beats creative**, and each arm falls through only on its
    /// own absence.
    #[test]
    fn selection_prefers_underwater_then_creative_then_default() {
        let full = BackgroundMusic::overworld().with_underwater(musics::under_water());
        assert_eq!(full.select(false, false).unwrap().sound, "minecraft:music.game");
        assert_eq!(
            full.select(true, false).unwrap().sound,
            "minecraft:music.creative"
        );
        assert_eq!(
            full.select(false, true).unwrap().sound,
            "minecraft:music.under_water"
        );
        // The pair that inverts if guessed: a creative player under water gets
        // the underwater track, not the creative one.
        assert_eq!(
            full.select(true, true).unwrap().sound,
            "minecraft:music.under_water"
        );

        // The Nether shape: a default only. Creative and underwater fall
        // through to it rather than to silence.
        let nether = BackgroundMusic::of_sound("minecraft:music.nether.crimson_forest");
        for (c, u) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(
                nether.select(c, u).unwrap().sound,
                "minecraft:music.nether.crimson_forest"
            );
        }

        // …and a record with a creative track but no default really is silent
        // for everyone else, which a fall-through-to-game reading would hide.
        let odd = BackgroundMusic {
            creative_music: Some(musics::creative()),
            ..BackgroundMusic::empty()
        };
        assert!(odd.select(false, false).is_none());
        assert_eq!(odd.select(true, false).unwrap().sound, "minecraft:music.creative");
    }

    /// A biome REPLACES the dimension's record rather than merging into it.
    #[test]
    fn a_biome_replaces_the_dimension_base_rather_than_merging() {
        let base = BackgroundMusic::overworld();
        let nether_biome = BackgroundMusic::of_sound("minecraft:music.nether.basalt_deltas");

        let inherited = BackgroundMusic::resolve(&base, None);
        assert_eq!(inherited, base, "no biome record means inherit");

        let overridden = BackgroundMusic::resolve(&base, Some(&nether_biome));
        assert_eq!(overridden, nether_biome);
        // The point of "replaces": the base's creative track does NOT survive,
        // so a creative player in a Nether biome hears the biome's music.
        assert!(overridden.creative_music.is_none());
        assert_eq!(
            overridden.select(true, false).unwrap().sound,
            "minecraft:music.nether.basalt_deltas"
        );
    }

    /// An explicitly empty record silences; an absent one inherits.
    #[test]
    fn empty_and_absent_are_different() {
        let base = BackgroundMusic::overworld();
        let explicit_empty = BackgroundMusic::empty();
        assert!(BackgroundMusic::resolve(&base, Some(&explicit_empty)).is_empty());
        assert!(!BackgroundMusic::resolve(&base, None).is_empty());
    }
}
