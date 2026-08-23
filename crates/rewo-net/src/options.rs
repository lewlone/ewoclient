//! `Options` — the two client options Rewo has and `options.txt` (M157).
//!
//! # Two options, not the three §0.0 named
//!
//! `REWO_PLAN.md` §0.0 listed three constants an options screen would unblock.
//! **`getMusicVolume` was never one of them.** `Minecraft.java:2623` is a screen
//! check followed by the `MUSIC_VOLUME` environment-attribute probe, with no
//! options entry anywhere in its path — so no options screen could have
//! unblocked it, and the claim it rested on ("no 26.2 biome declares it") was
//! false. **M154 fixed it as a bug.**
//!
//! What is left is genuinely two, and both really are `OptionInstance`s on
//! `Options.java` persisted to `options.txt`: `musicFrequency` (`:961-968`) and
//! `hideLightningFlash` (`:92-94`).
//!
//! # The file
//!
//! `new File(workingDirectory, "options.txt")` (`:1468`) — the **game**
//! directory, not a config dir. One `name:value` per line, split by
//! `Splitter.on(':').limit(2)` (`:85`), and the **limit is load-bearing**: a
//! value containing a colon survives, because only the first is a separator.
//! A line that does not split is skipped with a warning rather than failing the
//! load, so one bad entry cannot cost a player every other setting.
//!
//! The value is JSON. `option.codec().encodeStart(JsonOps.INSTANCE, ..)` on
//! write and `LenientJsonParser.parse` on read (`:1676-1680`, `:1782-1787`), so
//! a string-valued option is quoted in the file and a boolean is not.
//!
//! **`processOptions` is ONE method driving both load and save** (`:1543`),
//! which is why the format cannot drift between them — the same property this
//! module gets by having [`Options::apply_line`] and [`Options::to_lines`]
//! tested against each other rather than separately.

use crate::music::MusicFrequency;
use crate::sounds::SoundSource;

/// `options.txt`'s two entries that Rewo has (M157).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// `options.music_frequency` — how often a music track starts.
    pub music_frequency: MusicFrequency,
    /// `options.hideLightningFlashes`.
    ///
    /// **Two consumers, not one.** `LightmapRenderStateExtractor:57-65` gates
    /// the End flash's `skyFactor += intensity`, which is the one Rewo has;
    /// `ClientLevel.getSkyFlashTime()` (`:992-994`) returns 0 when the option is
    /// on and feeds two more attribute layers — the thunderstorm flash, driven
    /// by `LightningBolt.tick` rather than by a packet. Rewo has no lightning
    /// entity, so the second is recorded rather than wired.
    pub hide_lightning_flash: bool,
    /// `Options.soundSourceVolumes` (M173) — the eleven sliders, indexed by
    /// [`SoundSource::ordinal`]. File keys are `soundCategory_<name>` with
    /// the SINGULAR `getName()` strings (`soundCategory_block`, not
    /// `_blocks`), values a JSON double with a legacy BOOL alternative
    /// (`Codec.withAlternative(doubleRange(0,1), BOOL, b -> b ? 1 : 0)`).
    pub sound_volumes: [f32; 11],
}

impl Default for Options {
    /// Vanilla's declared defaults: `MusicFrequency.DEFAULT` and `false`.
    fn default() -> Self {
        Self {
            music_frequency: MusicFrequency::Default,
            hide_lightning_flash: false,
            // `createSoundSliderOptionInstance(.., 1.0, ..)` — every slider
            // starts at full.
            sound_volumes: [1.0; 11],
        }
    }
}

/// `options.music_frequency`'s key in the file.
pub const KEY_MUSIC_FREQUENCY: &str = "musicFrequency";
/// `options.hideLightningFlashes`' key in the file.
pub const KEY_HIDE_LIGHTNING: &str = "hideLightningFlashes";

/// `MusicFrequency.getSerializedName()` (`MusicManager.java:190-193`).
///
/// **The UPPERCASE first constructor argument, not the lowercase translation
/// key beside it on the same line.** `DEFAULT("DEFAULT", "options.music_
/// frequency.default", 20)` — the first is the serialized name and the second
/// is what the screen displays. Writing the translation key would produce a
/// file vanilla cannot read, and reading it would silently reset the option to
/// its default on every launch.
pub fn frequency_name(f: MusicFrequency) -> &'static str {
    match f {
        MusicFrequency::Default => "DEFAULT",
        MusicFrequency::Frequent => "FREQUENT",
        MusicFrequency::Constant => "CONSTANT",
    }
}

/// The inverse. `None` for a name no version of the enum has.
pub fn frequency_from_name(s: &str) -> Option<MusicFrequency> {
    match s {
        "DEFAULT" => Some(MusicFrequency::Default),
        "FREQUENT" => Some(MusicFrequency::Frequent),
        "CONSTANT" => Some(MusicFrequency::Constant),
        _ => None,
    }
}

/// The cycle order `OptionInstance.Enum` walks — `MusicFrequency.values()`.
pub const FREQUENCY_CYCLE: [MusicFrequency; 3] = [
    MusicFrequency::Default,
    MusicFrequency::Frequent,
    MusicFrequency::Constant,
];

/// A volume line's key — `"soundCategory_" + source.getName()`
/// (`Options.java:1627`). The names are the SINGULAR forms; see the test
/// pinning case-sensitivity.
pub fn volume_key(source: SoundSource) -> String {
    format!("soundCategory_{}", source.name())
}

/// The inverse — `None` for a key that is not a volume line.
pub fn volume_source(key: &str) -> Option<SoundSource> {
    SoundSource::from_name(key.strip_prefix("soundCategory_")?)
}

/// `OPTION_SPLITTER = Splitter.on(':').limit(2)` (`Options.java:85`).
///
/// **Only the FIRST colon separates**, so a value containing one survives
/// intact. Neither of Rewo's two options can produce such a value today, which
/// is exactly why this is a named function with its own test rather than an
/// inline `split_once`: a fixture built from the current options cannot tell
/// `split_once` from `split`, so a mutation swapping them survives — measured,
/// not assumed. Vanilla has several such values (`lang:en_us` does not, but a
/// resource-pack list and a key binding both can), and the next option added
/// here might.
pub fn split_line(line: &str) -> Option<(&str, &str)> {
    line.split_once(':')
}

impl Options {
    /// Apply one `name:value` line.
    ///
    /// Returns whether the line was understood. **A line that is not is
    /// skipped rather than fatal** (`:1652`, `LOGGER.warn("Skipping bad option:
    /// {}")`), so a single corrupt entry cannot cost a player every other
    /// setting — and neither can an option from a newer version.
    pub fn apply_line(&mut self, line: &str) -> bool {
        let Some((name, raw)) = split_line(line) else {
            return false;
        };
        match name {
            KEY_MUSIC_FREQUENCY => {
                // The value is JSON, so a string option is QUOTED in the file.
                // The quotes are stripped leniently rather than required:
                // `LenientJsonParser` accepts a bare word, and a file
                // hand-edited without them should not silently reset.
                let v = raw.trim().trim_matches('"');
                match frequency_from_name(v) {
                    Some(f) => {
                        self.music_frequency = f;
                        true
                    }
                    None => false,
                }
            }
            KEY_HIDE_LIGHTNING => match raw.trim() {
                "true" => {
                    self.hide_lightning_flash = true;
                    true
                }
                "false" => {
                    self.hide_lightning_flash = false;
                    true
                }
                _ => false,
            },
            key => match volume_source(key) {
                Some(source) => {
                    // `Codec.withAlternative(Codec.doubleRange(0.0, 1.0),
                    // Codec.BOOL, b -> b ? 1.0 : 0.0)` — a legacy boolean
                    // reads as full/off; an out-of-range double FAILS the
                    // parse (vanilla logs and keeps the default), it is not
                    // clamped.
                    let v = match raw.trim() {
                        "true" => Some(1.0),
                        "false" => Some(0.0),
                        t => t.parse::<f32>().ok().filter(|v| (0.0..=1.0).contains(v)),
                    };
                    match v {
                        Some(v) => {
                            self.sound_volumes[source.ordinal() as usize] = v;
                            true
                        }
                        None => false,
                    }
                }
                None => false,
            },
        }
    }

    /// One slider, by source.
    pub fn sound_volume(&self, source: SoundSource) -> f32 {
        self.sound_volumes[source.ordinal() as usize]
    }

    /// Store a slider value (the screen's write path). Clamped here because
    /// the mouse math can land epsilon outside `0..=1`; vanilla's
    /// `setValue` clamps before `applyValue` the same way.
    pub fn set_sound_volume(&mut self, source: SoundSource, v: f32) {
        self.sound_volumes[source.ordinal() as usize] = v.clamp(0.0, 1.0);
    }

    /// Parse a whole file. Unknown and malformed lines are skipped.
    pub fn parse(text: &str) -> Options {
        let mut o = Options::default();
        for line in text.lines() {
            o.apply_line(line);
        }
        o
    }

    /// The lines this writes back, in `processOptions` order.
    ///
    /// **Only the options Rewo has.** Vanilla's `save` rewrites the whole file
    /// from its own field set, which would DELETE every entry Rewo does not
    /// model — a vanilla client sharing the directory would lose its render
    /// distance, its keybinds and its volumes. So the caller merges rather than
    /// replaces; see [`Options::merge_into`].
    pub fn to_lines(self) -> Vec<String> {
        let mut out = vec![
            format!(
                "{KEY_MUSIC_FREQUENCY}:\"{}\"",
                frequency_name(self.music_frequency)
            ),
            format!("{KEY_HIDE_LIGHTNING}:{}", self.hide_lightning_flash),
        ];
        for source in SoundSource::ALL {
            // GSON prints a double with its decimal point (`1.0`, `0.5`);
            // Rust's `{}` prints `1` for `1.0f32`, so `{:?}` — the shortest
            // round-trip form, which for in-range volumes matches what a
            // vanilla client can read back.
            out.push(format!(
                "{}:{:?}",
                volume_key(source),
                self.sound_volumes[source.ordinal() as usize]
            ));
        }
        out
    }

    /// Rewrite `existing` with this option set, preserving every other line.
    ///
    /// **This is the difference between Rewo and vanilla's `save`, and it is
    /// deliberate.** Vanilla owns the whole file; Rewo models two of its
    /// roughly eighty entries, so a wholesale rewrite would silently discard
    /// the rest. A line whose key Rewo owns is replaced in place — keeping the
    /// file's order stable — and one it does not is copied through untouched.
    pub fn merge_into(self, existing: &str) -> String {
        let mine = self.to_lines();
        let mut owned: Vec<String> =
            vec![KEY_MUSIC_FREQUENCY.to_string(), KEY_HIDE_LIGHTNING.to_string()];
        for source in SoundSource::ALL {
            owned.push(volume_key(source));
        }
        let mut out: Vec<String> = Vec::new();
        let mut written = vec![false; owned.len()];
        for line in existing.lines() {
            let key = line.split_once(':').map(|(k, _)| k);
            match key.and_then(|k| owned.iter().position(|o| o == k)) {
                Some(i) => {
                    out.push(mine[i].clone());
                    written[i] = true;
                }
                None => out.push(line.to_string()),
            }
        }
        for (i, done) in written.iter().enumerate() {
            if !done {
                out.push(mine[i].clone());
            }
        }
        let mut s = out.join("\n");
        s.push('\n');
        s
    }

    /// `OptionInstance.Enum`'s cycle — the next value, wrapping.
    pub fn cycle_frequency(&mut self) {
        let i = FREQUENCY_CYCLE
            .iter()
            .position(|f| *f == self.music_frequency)
            .unwrap_or(0);
        self.music_frequency = FREQUENCY_CYCLE[(i + 1) % FREQUENCY_CYCLE.len()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The serialized name is the UPPERCASE constructor argument.**
    ///
    /// Writing the translation key beside it — the mistake the two literals on
    /// one line invite — produces a file vanilla cannot read, and reading it
    /// resets the option to its default on every launch, silently.
    #[test]
    fn the_serialized_name_is_not_the_translation_key() {
        assert_eq!(frequency_name(MusicFrequency::Default), "DEFAULT");
        assert_eq!(frequency_name(MusicFrequency::Frequent), "FREQUENT");
        assert_eq!(frequency_name(MusicFrequency::Constant), "CONSTANT");
        // The translation key is a DIFFERENT string on the same source line.
        assert!(!frequency_name(MusicFrequency::Default).contains("options."));
        for f in FREQUENCY_CYCLE {
            assert_eq!(frequency_from_name(frequency_name(f)), Some(f));
        }
        assert_eq!(frequency_from_name("options.music_frequency.default"), None);
        assert_eq!(frequency_from_name("default"), None, "case sensitive");
    }

    /// **`Splitter.on(':').limit(2)` — only the FIRST colon separates.**
    ///
    /// Splitting on every colon would truncate any value containing one. None
    /// of Rewo's two can today, which is exactly why this is worth pinning:
    /// the next option added might.
    /// **`Splitter.on(':').limit(2)` — only the FIRST colon separates.**
    ///
    /// Tested on [`split_line`] directly rather than through `apply_line`,
    /// because neither of Rewo's two option VALUES can contain a colon: an
    /// `apply_line` fixture rejects `musicFrequency:"A:B"` on its content
    /// whichever way the split works, so it cannot tell the two apart. A
    /// mutation swapping `split_once` for `split` survived exactly that test
    /// before this one existed.
    #[test]
    fn only_the_first_colon_separates() {
        assert_eq!(split_line("a:b"), Some(("a", "b")));
        assert_eq!(
            split_line("a:b:c"),
            Some(("a", "b:c")),
            "the value keeps every colon after the first"
        );
        assert_eq!(split_line("key_key.jump:key.keyboard.space").unwrap().1, "key.keyboard.space");
        assert_eq!(split_line("nocolon"), None);
        // An empty value is a value, not a missing one.
        assert_eq!(split_line("a:"), Some(("a", "")));

        let mut o = Options::default();
        assert!(o.apply_line("hideLightningFlashes:true"));
        assert!(o.hide_lightning_flash);
    }

    /// A bad line is SKIPPED rather than fatal, so one corrupt entry cannot
    /// cost a player every other setting — and neither can an option from a
    /// newer version of the game.
    #[test]
    fn a_bad_line_is_skipped_and_the_rest_still_load() {
        let o = Options::parse(
            "this line has no separator\n\
             musicFrequency:\"FREQUENT\"\n\
             quackFactor:17\n\
             hideLightningFlashes:notabool\n\
             hideLightningFlashes:true\n",
        );
        assert_eq!(o.music_frequency, MusicFrequency::Frequent);
        assert!(o.hide_lightning_flash, "the later good line still applied");
    }

    /// The file's values are JSON, so a string option is QUOTED — and a
    /// hand-edited file without the quotes still loads.
    #[test]
    fn a_string_option_is_quoted_and_a_bare_word_is_still_read() {
        assert_eq!(
            Options::default().to_lines()[0],
            "musicFrequency:\"DEFAULT\"",
            "encodeStart(JsonOps) quotes a string"
        );
        let mut o = Options::default();
        assert!(o.apply_line("musicFrequency:CONSTANT"));
        assert_eq!(o.music_frequency, MusicFrequency::Constant);
    }

    /// Load and save agree, which is the property vanilla gets structurally
    /// from `processOptions` being one method used by both.
    #[test]
    fn every_value_round_trips_through_the_file() {
        for f in FREQUENCY_CYCLE {
            for hide in [false, true] {
                let o = Options {
                    music_frequency: f,
                    hide_lightning_flash: hide,
                    sound_volumes: [1.0; 11],
                };
                assert_eq!(Options::parse(&o.to_lines().join("\n")), o);
            }
        }
    }

    /// **Rewo merges where vanilla rewrites, and dropping that would delete a
    /// vanilla client's settings.**
    ///
    /// Vanilla's `save` regenerates the whole file from its own ~80 fields.
    /// Rewo models two of them, so a wholesale rewrite of a shared game
    /// directory would silently discard render distance, keybinds and volumes.
    /// The file keys use the SINGULAR `getName()` strings, case-sensitively —
    /// `soundCategory_block`, never `_blocks`/`_Block` (`Options.java:1627`;
    /// the name table's own test pins the same fact from the other side).
    #[test]
    fn volume_keys_are_singular_and_case_sensitive() {
        let mut o = Options::default();
        assert!(o.apply_line("soundCategory_block:0.5"));
        assert_eq!(o.sound_volume(SoundSource::Blocks), 0.5);
        assert!(!o.apply_line("soundCategory_blocks:0.5"), "plural rejected");
        assert!(!o.apply_line("soundCategory_Block:0.5"), "case-sensitive");
    }

    /// `Codec.withAlternative(doubleRange(0,1), BOOL, b -> b ? 1 : 0)` — a
    /// legacy boolean reads as full/off; an out-of-range double FAILS the
    /// parse and the default survives (vanilla logs + keeps default; it does
    /// NOT clamp).
    #[test]
    fn volume_values_accept_legacy_bools_and_reject_out_of_range() {
        let mut o = Options::default();
        assert!(o.apply_line("soundCategory_music:false"));
        assert_eq!(o.sound_volume(SoundSource::Music), 0.0);
        assert!(o.apply_line("soundCategory_music:true"));
        assert_eq!(o.sound_volume(SoundSource::Music), 1.0);
        assert!(!o.apply_line("soundCategory_music:1.5"), "above range rejected");
        assert_eq!(o.sound_volume(SoundSource::Music), 1.0, "default kept, not clamped");
        assert!(!o.apply_line("soundCategory_music:-0.1"));
    }

    /// The eleven volumes round-trip through `to_lines` + `parse`, at values
    /// with decimal points — the writer must print `1.0` (GSON's double
    /// form), not Rust's bare `1`.
    #[test]
    fn volumes_round_trip_with_decimal_points() {
        let mut o = Options::default();
        o.set_sound_volume(SoundSource::Master, 0.5);
        o.set_sound_volume(SoundSource::Ui, 0.25);
        let text = o.to_lines().join("
");
        assert!(text.contains("soundCategory_master:0.5"), "{text}");
        assert!(text.contains("soundCategory_ui:0.25"));
        assert!(
            text.contains("soundCategory_weather:1.0"),
            "a full slider writes 1.0, not 1 — {text}"
        );
        assert_eq!(Options::parse(&text), o);
    }

    /// The screen's write path clamps (the mouse math can land epsilon
    /// outside), where the FILE path rejects — two different rules on
    /// purpose, vanilla's `setValue` vs its codec.
    #[test]
    fn set_sound_volume_clamps_where_the_file_rejects() {
        let mut o = Options::default();
        o.set_sound_volume(SoundSource::Voice, 1.2);
        assert_eq!(o.sound_volume(SoundSource::Voice), 1.0);
        o.set_sound_volume(SoundSource::Voice, -0.2);
        assert_eq!(o.sound_volume(SoundSource::Voice), 0.0);
    }

    #[test]
    fn saving_preserves_every_line_rewo_does_not_model() {
        let existing = "version:4189\n\
                        renderDistance:12\n\
                        musicFrequency:\"DEFAULT\"\n\
                        key_key.jump:key.keyboard.space\n";
        let o = Options {
            music_frequency: MusicFrequency::Constant,
            hide_lightning_flash: true,
            sound_volumes: [1.0; 11],
        };
        let out = o.merge_into(existing);
        assert!(out.contains("renderDistance:12"), "an unmodelled option survived");
        assert!(out.contains("key_key.jump:key.keyboard.space"));
        assert!(out.contains("version:4189"));
        assert!(out.contains("musicFrequency:\"CONSTANT\""), "and ours updated");
        assert!(
            out.contains("hideLightningFlashes:true"),
            "one we own but the file lacked is appended"
        );
        // Replaced IN PLACE rather than appended, so the file's order is stable
        // across saves and a diff stays readable.
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[2], "musicFrequency:\"CONSTANT\"");
        assert_eq!(Options::parse(&out), o, "and it reads back as itself");
    }

    /// **The option REACHES the manager, and it re-rolls when it does.**
    ///
    /// This exists because "the parse is right and the resolve is right and
    /// nothing checks they reach the consumer" is the shape M154's battery
    /// caught and M97 named — five times now in this project. The observable
    /// is `setMinutesBetweenSongs` doing more than storing a field: it
    /// immediately re-rolls `nextSongDelay` from the new frequency, so the
    /// delay changes on the call rather than at the next track.
    #[test]
    fn setting_the_frequency_reschedules_the_next_track_now() {
        let mut m = crate::music::MusicManager::new(7, MusicFrequency::Default);
        let before = m.next_song_delay();
        m.set_frequency(MusicFrequency::Constant, None);
        let after = m.next_song_delay();
        assert_ne!(
            before, after,
            "set_frequency must re-roll the delay, not just store the value"
        );
        // CONSTANT's own maximum is zero minutes, so with nothing on offer the
        // next track is due immediately — which is the branch a client with no
        // situational music takes.
        assert_eq!(after, 0);
    }

    /// **Seeding from `options.txt` must NOT re-roll the delay** (M161).
    ///
    /// The pair with the test above is the whole point: vanilla's constructor
    /// (`MusicManager.java:30`) plainly stores the option and leaves
    /// `nextSongDelay` at its field initialiser of 100, while
    /// `setMinutesBetweenSongs` (`:154-155`) stores it AND re-rolls. M157 seeded
    /// through the second, so every session started with a delay of up to 24000
    /// against a 100-tick `STARTING_DELAY` — and since the delay is decremented
    /// once per tick, **no music played in any session under ~20 minutes**.
    ///
    /// One test alone cannot express this: "re-rolls" and "does not re-roll"
    /// are the same assertion with the sign flipped, and only the pair says
    /// which path belongs where.
    #[test]
    fn seeding_the_frequency_keeps_the_starting_delay() {
        let mut m = crate::music::MusicManager::new(7, MusicFrequency::Default);
        let before = m.next_song_delay();
        assert_eq!(before, crate::music::STARTING_DELAY, "a fresh manager waits 100");
        m.seed_frequency(MusicFrequency::Constant);
        assert_eq!(
            m.next_song_delay(),
            crate::music::STARTING_DELAY,
            "seeding stores the value and leaves the delay alone; re-rolling here              is what stopped music playing at all"
        );
        // …and the value really was taken, so this is not passing by doing nothing.
        m.set_frequency(MusicFrequency::Constant, None);
        assert_ne!(
            m.next_song_delay(),
            crate::music::STARTING_DELAY,
            "the mid-session path still re-rolls"
        );
    }

    /// The cycle is `MusicFrequency.values()` order and it wraps.
    #[test]
    fn the_cycle_wraps_through_values_order() {
        let mut o = Options::default();
        assert_eq!(o.music_frequency, MusicFrequency::Default);
        o.cycle_frequency();
        assert_eq!(o.music_frequency, MusicFrequency::Frequent);
        o.cycle_frequency();
        assert_eq!(o.music_frequency, MusicFrequency::Constant);
        o.cycle_frequency();
        assert_eq!(o.music_frequency, MusicFrequency::Default, "and wraps");
    }
}
