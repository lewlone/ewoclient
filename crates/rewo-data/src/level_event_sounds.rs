//! `level_event` → sound event (M66).
//!
//! M63 decoded `sound`, `sound_entity` and `stop_sound` and stopped, noting
//! that `level_event` "carries most of vanilla's block/world sounds in its id
//! table". This is that table. **Most block interactions never send a `sound`
//! packet at all** — a dispenser firing, an anvil landing, a composter
//! filling, a zombie battering a door, a trial spawner arming all arrive as a
//! `level_event` id, and a client that only listens to the three sound packets
//! plays none of them.
//!
//! Data only, like everything in this crate's audio layer: an id becomes a
//! registry name plus the arguments vanilla passes to `playLocalSound`.
//! Turning that into a noise is a mixer's job.
//!
//! ## The packet is already decoded — this is the missing half
//!
//! `rewo_net::route_level_event` reads the body (`i32 type`, `BlockPos pos`,
//! `i32 data`, `bool globalEvent`) and keeps only 2001, whose `data` is a
//! block-state id, for M37's particle path. Everything else it drops. So the
//! gap was never the decode; it was that nothing knew what any other id
//! *meant*.
//!
//! ## Two dispatch tables, not one
//!
//! `ClientPacketListener.handleLevelEvent` branches on the packet's
//! `globalEvent` flag: true routes to `globalLevelEvent`, false to
//! `levelEvent`. They are **disjoint switches**. `globalLevelEvent` handles
//! exactly 1023, 1028 and 1038 and nothing else; `levelEvent` handles
//! everything else and *not* those three. So a 1023 arriving with the flag
//! clear is silence, and so is a 1000 arriving with it set — which is why
//! [`resolve`] takes the flag and matches on it rather than treating it as a
//! hint. The three global events are also **not placed at `pos`**: vanilla
//! puts them two blocks from the camera along the direction to `pos`, so a
//! wither spawning across the world is heard at full volume from the right
//! bearing. That is what [`Placement::Camera`] records.
//!
//! ## What is deliberately absent
//!
//! Three ids play a sound whose *identity is not in this table*, because
//! vanilla derives it from something the packet alone does not name. They are
//! listed in [`DERIVED`] rather than guessed at, because a plausible guess
//! here is worse than a gap: it would play the wrong sound confidently.
//!
//! Seventeen more ids are handled by `levelEvent` and emit **no** sound at all
//! — particles, or a stop. They are in [`SILENT`], so "this id is not in
//! `SOUNDS`" can be told apart from "nobody has looked at this id yet". The
//! three lists partition `LevelEvent`'s 83 constants exactly, and a test says
//! so.
//!
//! ## Pitch is not here, and volume is only mostly here
//!
//! Over thirty rows randomise pitch — `(random.nextFloat() -
//! random.nextFloat()) * 0.2F + 1.0F` and friends — off `ClientLevel.random`,
//! a generator with no seed on the wire. It is per-client jitter by
//! construction, so there is no "correct" value to transcribe and a playback
//! layer should draw its own. Volume *is* a literal in every row but two, and
//! it is far from decorative: a ghast fireball is 10.0 and a bat taking off is
//! 0.05, a factor of two hundred. A mixer that assumed 1.0 everywhere would be
//! wrong in a way only a listener can hear, which is why it is carried.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/world/level/block/LevelEvent.java` — the id constants
//! - `net/minecraft/client/renderer/LevelEventHandler.java` — both switches
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleLevelEvent`
//! - `net/minecraft/sounds/SoundEvents.java` — every `register("…")` name
//! - `net/minecraft/world/level/block/ComposterBlock.java` — 1500's body
//! - `net/minecraft/client/resources/sounds/SimpleSoundInstance.java` —
//!   `forLocalAmbience`, 1032's route

/// Which `data` values a row applies to.
///
/// Four ids branch on `data` and play a *different* sound or volume per
/// branch, so an id can own more than one row. Everything else is
/// [`DataGate::Always`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataGate {
    Always,
    Eq(i32),
    Ne(i32),
    /// `data > n`.
    Gt(i32),
    /// `data <= n` — the other half of a `Gt`.
    Le(i32),
}

impl DataGate {
    pub fn matches(self, data: i32) -> bool {
        match self {
            DataGate::Always => true,
            DataGate::Eq(n) => data == n,
            DataGate::Ne(n) => data != n,
            DataGate::Gt(n) => data > n,
            DataGate::Le(n) => data <= n,
        }
    }
}

/// Where vanilla puts the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// `level.playLocalSound(pos, …)` — at the block.
    Block,
    /// Two blocks from the camera along the direction to `pos`. Only the
    /// three `globalLevelEvent` ids, and only once the camera is initialised.
    Camera,
    /// `SimpleSoundInstance.forLocalAmbience` — attached to the listener, so
    /// it does not attenuate or pan at all.
    Listener,
}

/// One `(id, data)` → sound mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelEventSound {
    /// The `LevelEvent` constant.
    pub id: i32,
    /// A fully-qualified `minecraft:sound_event` registry name — feed it to
    /// [`crate::sound_events::SoundEvents::id_of`] or straight to
    /// [`crate::sounds_json::SoundsIndex::get_sound`].
    pub sound: &'static str,
    /// `SoundSource.getName()`, which `rewo_net::sounds::SoundSource::name`
    /// also returns. A string rather than the enum because that enum lives in
    /// `rewo-net`, and **`rewo-net` depends on `rewo-data`**, not the other
    /// way round; `SoundSource::from_name` is the join, and a test over there
    /// checks every row resolves.
    pub source: &'static str,
    /// `None` where vanilla computes one — see the module docs.
    pub volume: Option<f32>,
    pub data: DataGate,
    pub placement: Placement,
    /// `playLocalSound`'s trailing `distanceDelay`. When set, vanilla holds
    /// the sound by `sqrt(distanceSqr) / 40` seconds if it is more than 10
    /// blocks away — the thunder-crack effect, and the reason a trial spawner
    /// across a room sounds late rather than instant.
    pub distance_delay: bool,
    /// Only for ids routed through `globalLevelEvent`; see the module docs on
    /// why this is matched exactly rather than ignored.
    pub global: bool,
    /// A condition or detail the fields above cannot express. `None` means the
    /// row applies whenever `id` and `data` match.
    pub note: Option<&'static str>,
}

/// Shorthand for the overwhelmingly common shape: at the block, no delay, no
/// gate, a literal volume.
const fn at_block(id: i32, sound: &'static str, source: &'static str, volume: f32) -> LevelEventSound {
    LevelEventSound {
        id,
        sound,
        source,
        volume: Some(volume),
        data: DataGate::Always,
        placement: Placement::Block,
        distance_delay: false,
        global: false,
        note: None,
    }
}

/// Same, but with `distanceDelay` set — the trial-spawner / vault family.
const fn delayed(id: i32, sound: &'static str, source: &'static str, volume: f32) -> LevelEventSound {
    LevelEventSound {
        distance_delay: true,
        ..at_block(id, sound, source, volume)
    }
}

/// Every `level_event` id whose sound this table can name.
///
/// Rows are in id order, and an id with a `data` branch owns consecutive rows
/// whose gates are exhaustive over the branch vanilla takes. `SoundSource`
/// strings are `SoundSource.getName()`, so `"block"` is `BLOCKS` and
/// `"player"` is `PLAYERS`.
pub const SOUNDS: &[LevelEventSound] = &[
    // -- 1000s: the sound-only block/mob events ---------------------------
    at_block(1000, "minecraft:block.dispenser.dispense", "block", 1.0),
    at_block(1001, "minecraft:block.dispenser.fail", "block", 1.0),
    at_block(1002, "minecraft:block.dispenser.launch", "block", 1.0),
    at_block(1004, "minecraft:entity.firework_rocket.shoot", "neutral", 1.0),
    // 1009 is two different sounds, and **only** for data 0 and 1: vanilla's
    // `if/else if` has no `else`, so any other value is silence rather than a
    // fall-through to the first branch.
    LevelEventSound {
        data: DataGate::Eq(0),
        ..at_block(1009, "minecraft:block.fire.extinguish", "block", 0.5)
    },
    LevelEventSound {
        data: DataGate::Eq(1),
        ..at_block(1009, "minecraft:entity.generic.extinguish_fire", "block", 0.7)
    },
    at_block(1015, "minecraft:entity.ghast.warn", "hostile", 10.0),
    at_block(1016, "minecraft:entity.ghast.shoot", "hostile", 10.0),
    at_block(1017, "minecraft:entity.ender_dragon.shoot", "hostile", 10.0),
    at_block(1018, "minecraft:entity.blaze.shoot", "hostile", 2.0),
    at_block(1019, "minecraft:entity.zombie.attack_wooden_door", "hostile", 2.0),
    at_block(1020, "minecraft:entity.zombie.attack_iron_door", "hostile", 2.0),
    at_block(1021, "minecraft:entity.zombie.break_wooden_door", "hostile", 2.0),
    at_block(1022, "minecraft:entity.wither.break_block", "hostile", 2.0),
    LevelEventSound {
        placement: Placement::Camera,
        global: true,
        note: Some("globalLevelEvent; skipped until the camera is initialised"),
        ..at_block(1023, "minecraft:entity.wither.spawn", "hostile", 1.0)
    },
    at_block(1024, "minecraft:entity.wither.shoot", "hostile", 2.0),
    at_block(1025, "minecraft:entity.bat.takeoff", "neutral", 0.05),
    at_block(1026, "minecraft:entity.zombie.infect", "hostile", 2.0),
    at_block(1027, "minecraft:entity.zombie_villager.converted", "hostile", 2.0),
    LevelEventSound {
        placement: Placement::Camera,
        global: true,
        note: Some("globalLevelEvent; skipped until the camera is initialised"),
        ..at_block(1028, "minecraft:entity.ender_dragon.death", "hostile", 5.0)
    },
    at_block(1029, "minecraft:block.anvil.destroy", "block", 1.0),
    at_block(1030, "minecraft:block.anvil.use", "block", 1.0),
    at_block(1031, "minecraft:block.anvil.land", "block", 0.3),
    // The one row that is neither `playLocalSound` nor global.
    // `forLocalAmbience(sound, pitch, volume)` takes **pitch first**: the call
    // is `forLocalAmbience(PORTAL_TRAVEL, random * 0.4F + 0.8F, 0.25F)`, so
    // 0.25 is the volume and the random term is the pitch. Reading the
    // argument list left to right as (volume, pitch) gives a portal that
    // roars at three times the volume at a fixed pitch.
    LevelEventSound {
        placement: Placement::Listener,
        ..at_block(1032, "minecraft:block.portal.travel", "ambient", 0.25)
    },
    at_block(1033, "minecraft:block.chorus_flower.grow", "block", 1.0),
    at_block(1034, "minecraft:block.chorus_flower.death", "block", 1.0),
    at_block(1035, "minecraft:block.brewing_stand.brew", "block", 1.0),
    LevelEventSound {
        placement: Placement::Camera,
        global: true,
        note: Some("globalLevelEvent; skipped until the camera is initialised"),
        ..at_block(1038, "minecraft:block.end_portal.spawn", "hostile", 1.0)
    },
    at_block(1039, "minecraft:entity.phantom.bite", "hostile", 0.3),
    at_block(1040, "minecraft:entity.zombie.converted_to_drowned", "hostile", 2.0),
    at_block(1041, "minecraft:entity.husk.converted_to_zombie", "hostile", 2.0),
    at_block(1042, "minecraft:block.grindstone.use", "block", 1.0),
    at_block(1043, "minecraft:item.book.page_turn", "block", 1.0),
    at_block(1044, "minecraft:block.smithing_table.use", "block", 1.0),
    at_block(1045, "minecraft:block.pointed_dripstone.land", "block", 2.0),
    at_block(
        1046,
        "minecraft:block.pointed_dripstone.drip_lava_into_cauldron",
        "block",
        2.0,
    ),
    at_block(
        1047,
        "minecraft:block.pointed_dripstone.drip_water_into_cauldron",
        "block",
        2.0,
    ),
    at_block(1048, "minecraft:entity.skeleton.converted_to_stray", "hostile", 2.0),
    at_block(1049, "minecraft:block.crafter.craft", "block", 1.0),
    at_block(1050, "minecraft:block.crafter.fail", "block", 1.0),
    at_block(1051, "minecraft:entity.wind_charge.throw", "block", 0.5),
    at_block(1052, "minecraft:block.sulfur_spike.land", "block", 2.0),
    // -- 1500s: particles-with-a-sound ------------------------------------
    // `ComposterBlock.handleFill(level, pos, data > 0)` picks the sound, so
    // the branch is on `data > 0` and not on a specific value.
    LevelEventSound {
        data: DataGate::Gt(0),
        ..at_block(1500, "minecraft:block.composter.fill_success", "block", 1.0)
    },
    LevelEventSound {
        data: DataGate::Le(0),
        ..at_block(1500, "minecraft:block.composter.fill", "block", 1.0)
    },
    at_block(1501, "minecraft:block.lava.extinguish", "block", 0.5),
    at_block(1502, "minecraft:block.redstone_torch.burnout", "block", 0.5),
    at_block(1503, "minecraft:block.end_portal_frame.fill", "block", 1.0),
    at_block(1505, "minecraft:item.bone_meal.use", "block", 1.0),
    // -- 2000s ------------------------------------------------------------
    // 2002 and 2007 share one `case` label, so they play the same sound; the
    // `data` colour and the particle type are what differ.
    at_block(2002, "minecraft:entity.splash_potion.break", "neutral", 1.0),
    LevelEventSound {
        data: DataGate::Eq(1),
        ..at_block(2006, "minecraft:entity.dragon_fireball.explode", "hostile", 1.0)
    },
    at_block(2007, "minecraft:entity.splash_potion.break", "neutral", 1.0),
    // -- 3000s ------------------------------------------------------------
    at_block(3000, "minecraft:block.end_gateway.spawn", "block", 10.0),
    at_block(3001, "minecraft:entity.ender_dragon.growl", "hostile", 64.0),
    at_block(3003, "minecraft:item.honeycomb.wax_on", "block", 1.0),
    // Both branches of 3006 play the same sound, so the id resolves even
    // though the volume and the *whether* do not: with `data >> 6 == 0` it is
    // an unconditional 1.0, and otherwise it fires with probability
    // `0.3 + count * 0.1` at `0.15 + 0.02 * count^2 * random`.
    LevelEventSound {
        volume: None,
        note: Some(
            "volume 1.0 when data >> 6 == 0; otherwise probabilistic \
             (0.3 + count * 0.1) at a computed volume",
        ),
        ..at_block(3006, "minecraft:block.sculk.charge", "block", 1.0)
    },
    LevelEventSound {
        note: Some(
            "silent when the shrieker is waterlogged; placed at \
             pos + (0.5, SculkShriekerBlock.TOP_Y, 0.5), not at the block corner",
        ),
        ..at_block(3007, "minecraft:block.sculk_shrieker.shriek", "block", 2.0)
    },
    delayed(3012, "minecraft:block.trial_spawner.spawn_mob", "block", 1.0),
    delayed(3013, "minecraft:block.trial_spawner.detect_player", "block", 1.0),
    delayed(3014, "minecraft:block.trial_spawner.eject_item", "block", 1.0),
    LevelEventSound {
        note: Some("only when the block entity at pos is a vault"),
        ..delayed(3015, "minecraft:block.vault.activate", "block", 1.0)
    },
    delayed(3016, "minecraft:block.vault.deactivate", "block", 1.0),
    delayed(3018, "minecraft:block.cobweb.place", "block", 1.0),
    // 3019 is 3013's ominous twin and plays the same sound; only the particle
    // type differs.
    delayed(3019, "minecraft:block.trial_spawner.detect_player", "block", 1.0),
    LevelEventSound {
        data: DataGate::Eq(0),
        ..delayed(3020, "minecraft:block.trial_spawner.ominous_activate", "block", 0.3)
    },
    LevelEventSound {
        data: DataGate::Ne(0),
        ..delayed(3020, "minecraft:block.trial_spawner.ominous_activate", "block", 1.0)
    },
    delayed(3021, "minecraft:block.trial_spawner.spawn_item", "block", 1.0),
];

/// An id that plays a sound whose identity this table cannot supply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DerivedLevelEventSound {
    pub id: i32,
    /// `SoundSource.getName()` of the sound vanilla plays, which *is* fixed
    /// even though the event is not.
    pub source: &'static str,
    /// What the sound is derived from, so a caller knows what it would need.
    pub why: &'static str,
}

/// The three ids [`SOUNDS`] deliberately omits.
///
/// Each plays a real sound; none of them names a fixed `SoundEvent`. Inventing
/// a plausible entry — `block.stone.break` for 2001, say — would be silently
/// wrong for every block that is not stone, which is worse than being absent.
pub const DERIVED: &[DerivedLevelEventSound] = &[
    DerivedLevelEventSound {
        id: 1010,
        source: "record",
        why: "the song is `registryAccess().lookupOrThrow(JUKEBOX_SONG).get(data)`, \
              a datapack registry the server sends in Configuration; its \
              `soundEvent` is the sound. Also, the song description goes on the \
              HUD, and the instance is held so 1011 can stop it.",
    },
    DerivedLevelEventSound {
        id: 2001,
        source: "block",
        why: "`Block.stateById(data).getSoundType().getBreakSound()`, so every \
              block has its own; volume is `(soundType.volume + 1) / 2` and pitch \
              `soundType.pitch * 0.8`. An air state plays nothing. Needs the \
              per-block-state SoundType table, which the bake does not carry.",
    },
    DerivedLevelEventSound {
        id: 3008,
        source: "player",
        why: "`((BrushableBlock) Block.stateById(data).getBlock()).getBrushCompletedSound()`, \
              a per-block field (suspicious sand vs gravel). A non-brushable \
              state plays nothing.",
    },
];

/// Ids `levelEvent` handles that emit no sound at all — particles, or a stop.
///
/// Present so "absent from [`SOUNDS`]" can be told from "unexamined". 1011 is
/// the odd one: it *stops* the jukebox instance 1010 started, which is a sound
/// operation with no sound event.
pub const SILENT: &[i32] = &[
    1011, // stop the jukebox song at pos
    1504, // dripstone drip particle
    2000, // smoke, shot along a face
    2003, // eye-of-ender death
    2004, // mob-block spawn
    2008, // dragon block break
    2009, // water evaporating
    2010, // white smoke, shot along a face
    2011, // bee growth
    2012, // turtle egg placement
    2013, // smash attack
    3002, // electric spark
    3004, // wax off
    3005, // scrape
    3009, // egg crack
    3011, // trial spawner spawn
    3017, // vault eject item
];

/// Every constant in `LevelEvent`, in declaration order.
///
/// Here so the three lists above can be checked to partition it. Without this
/// there is no way to tell a deliberate omission from a missed `case`, which
/// is the failure mode a hand-transcribed 83-entry switch actually has.
pub const ALL_IDS: &[i32] = &[
    1000, 1001, 1002, 1004, 1009, 1010, 1011, 1015, 1016, 1017, 1018, 1019, 1020, 1021, 1022,
    1023, 1024, 1025, 1026, 1027, 1028, 1029, 1030, 1031, 1032, 1033, 1034, 1035, 1038, 1039,
    1040, 1041, 1042, 1043, 1044, 1045, 1046, 1047, 1048, 1049, 1050, 1051, 1052, 1500, 1501,
    1502, 1503, 1504, 1505, 2000, 2001, 2002, 2003, 2004, 2006, 2007, 2008, 2009, 2010, 2011,
    2012, 2013, 3000, 3001, 3002, 3003, 3004, 3005, 3006, 3007, 3008, 3009, 3011, 3012, 3013,
    3014, 3015, 3016, 3017, 3018, 3019, 3020, 3021,
];

/// The sound a `level_event` packet asks for, or `None`.
///
/// `global` is the packet's own flag, and it is matched rather than ignored:
/// the two switches are disjoint, so a mismatched flag is silence in vanilla
/// too. `None` means one of: an unknown id, a silent id, a [`DERIVED`] id, or
/// a `data` value outside every branch of a gated id (1009 with `data == 2`,
/// for instance).
pub fn resolve(id: i32, data: i32, global: bool) -> Option<&'static LevelEventSound> {
    SOUNDS
        .iter()
        .find(|s| s.id == id && s.global == global && s.data.matches(data))
}

/// Every row for an id, whatever the `data`. For a caller enumerating what an
/// id can do rather than answering one packet.
pub fn rows_for(id: i32) -> impl Iterator<Item = &'static LevelEventSound> {
    SOUNDS.iter().filter(move |s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The completeness witness: the three lists partition `LevelEvent`'s
    /// constants, with no id in two of them and none left over. A missed
    /// `case` in the 660-line switch lands here rather than as silence
    /// somebody notices a year later.
    #[test]
    fn the_three_lists_partition_every_level_event_constant() {
        let named: HashSet<i32> = SOUNDS.iter().map(|s| s.id).collect();
        let derived: HashSet<i32> = DERIVED.iter().map(|d| d.id).collect();
        let silent: HashSet<i32> = SILENT.iter().copied().collect();
        let all: HashSet<i32> = ALL_IDS.iter().copied().collect();

        assert_eq!(all.len(), ALL_IDS.len(), "ALL_IDS has a duplicate");
        assert_eq!(silent.len(), SILENT.len(), "SILENT has a duplicate");
        assert!(named.is_disjoint(&derived));
        assert!(named.is_disjoint(&silent));
        assert!(derived.is_disjoint(&silent));

        let covered: HashSet<i32> = named.union(&derived).copied().collect::<HashSet<_>>();
        let covered: HashSet<i32> = covered.union(&silent).copied().collect();
        let missing: Vec<i32> = all.difference(&covered).copied().collect();
        let extra: Vec<i32> = covered.difference(&all).copied().collect();
        assert!(missing.is_empty(), "unclassified level events: {missing:?}");
        assert!(extra.is_empty(), "not a LevelEvent constant: {extra:?}");

        // The measured shape, so a change has to be deliberate.
        assert_eq!(all.len(), 83);
        assert_eq!(named.len(), 63);
        assert_eq!(SOUNDS.len(), 66, "63 ids, three of which own two rows");
        assert_eq!(derived.len(), 3);
        assert_eq!(silent.len(), 17);
    }

    /// Every row names a real sound event and a real category, in the exact
    /// spellings the rest of the stack uses.
    #[test]
    fn every_row_is_well_formed() {
        const SOURCES: &[&str] = &[
            "master", "music", "record", "weather", "block", "hostile", "neutral", "player",
            "ambient", "voice", "ui",
        ];
        for s in SOUNDS {
            assert!(
                s.sound.starts_with("minecraft:"),
                "{}: {} is not fully qualified",
                s.id,
                s.sound
            );
            assert!(
                SOURCES.contains(&s.source),
                "{}: {} is not a SoundSource name",
                s.id,
                s.source
            );
            if let Some(v) = s.volume {
                assert!(v > 0.0, "{}: volume {v}", s.id);
            }
        }
        for d in DERIVED {
            assert!(SOURCES.contains(&d.source), "{}: {}", d.id, d.source);
            assert!(!d.why.is_empty());
        }
    }

    /// Rows are in id order, so the table can be read against the decompiled
    /// switch top to bottom.
    #[test]
    fn rows_are_in_id_order() {
        assert!(SOUNDS.windows(2).all(|w| w[0].id <= w[1].id));
        assert!(ALL_IDS.windows(2).all(|w| w[0] < w[1]));
        assert!(SILENT.windows(2).all(|w| w[0] < w[1]));
    }

    /// The gated ids are exhaustive over nothing more than vanilla's own
    /// branches — and at most one row can match a given `data`, or `resolve`
    /// would be choosing arbitrarily.
    #[test]
    fn at_most_one_row_matches_any_id_and_data() {
        let ids: HashSet<i32> = SOUNDS.iter().map(|s| s.id).collect();
        for id in ids {
            for data in -3..=70 {
                for global in [false, true] {
                    let n = SOUNDS
                        .iter()
                        .filter(|s| s.id == id && s.global == global && s.data.matches(data))
                        .count();
                    assert!(n <= 1, "{id} data {data} global {global} matched {n} rows");
                }
            }
        }
    }

    /// 1009's two branches, and the fact that vanilla's `if/else if` has no
    /// `else`: `data == 2` is silence, not a fall back to the first branch.
    #[test]
    fn extinguish_fire_branches_on_data_and_has_no_fallback() {
        assert_eq!(
            resolve(1009, 0, false).unwrap().sound,
            "minecraft:block.fire.extinguish"
        );
        assert_eq!(resolve(1009, 0, false).unwrap().volume, Some(0.5));
        assert_eq!(
            resolve(1009, 1, false).unwrap().sound,
            "minecraft:entity.generic.extinguish_fire"
        );
        assert_eq!(resolve(1009, 1, false).unwrap().volume, Some(0.7));
        assert!(resolve(1009, 2, false).is_none());
        assert!(resolve(1009, -1, false).is_none());
    }

    /// The composter branch is `data > 0`, so every non-positive value —
    /// including a negative one — is the failure sound. A `data == 0` test
    /// alone would pass against an `Eq(0)` transcription too.
    #[test]
    fn the_composter_branches_on_data_being_positive() {
        assert_eq!(
            resolve(1500, 1, false).unwrap().sound,
            "minecraft:block.composter.fill_success"
        );
        assert_eq!(
            resolve(1500, 7, false).unwrap().sound,
            "minecraft:block.composter.fill_success"
        );
        assert_eq!(
            resolve(1500, 0, false).unwrap().sound,
            "minecraft:block.composter.fill"
        );
        assert_eq!(
            resolve(1500, -1, false).unwrap().sound,
            "minecraft:block.composter.fill"
        );
    }

    /// 3020 keeps one sound and changes only its volume, and the quiet branch
    /// is the `data == 0` one.
    #[test]
    fn the_ominous_activate_volume_depends_on_data() {
        let quiet = resolve(3020, 0, false).unwrap();
        let loud = resolve(3020, 1, false).unwrap();
        assert_eq!(quiet.sound, loud.sound);
        assert_eq!(quiet.volume, Some(0.3));
        assert_eq!(loud.volume, Some(1.0));
        assert_eq!(resolve(3020, -5, false).unwrap().volume, Some(1.0));
    }

    /// The two switches are disjoint: the global ids resolve only with the
    /// flag set, and every other id only with it clear.
    #[test]
    fn the_global_flag_selects_between_two_disjoint_switches() {
        for id in [1023, 1028, 1038] {
            assert!(resolve(id, 0, true).is_some(), "{id} global");
            assert!(resolve(id, 0, false).is_none(), "{id} not global");
        }
        for id in [1000, 1032, 2002, 3021] {
            assert!(resolve(id, 0, false).is_some(), "{id} local");
            assert!(resolve(id, 0, true).is_none(), "{id} global");
        }
        // …and the global rows are exactly the camera-placed ones.
        let global: HashSet<i32> = SOUNDS.iter().filter(|s| s.global).map(|s| s.id).collect();
        let camera: HashSet<i32> = SOUNDS
            .iter()
            .filter(|s| s.placement == Placement::Camera)
            .map(|s| s.id)
            .collect();
        assert_eq!(global, camera);
        assert_eq!(global, HashSet::from([1023, 1028, 1038]));
    }

    /// The portal's arguments, which `forLocalAmbience(sound, pitch, volume)`
    /// makes easy to swap. 0.25 is the volume; if this ever reads 0.8 or 1.2
    /// somebody read the call left to right.
    #[test]
    fn the_portal_travel_row_takes_volume_from_the_third_argument() {
        let s = resolve(1032, 0, false).unwrap();
        assert_eq!(s.volume, Some(0.25));
        assert_eq!(s.source, "ambient");
        assert_eq!(s.placement, Placement::Listener);
        // The only listener-placed row there is.
        assert_eq!(
            SOUNDS
                .iter()
                .filter(|s| s.placement == Placement::Listener)
                .count(),
            1
        );
    }

    /// `distanceDelay` is set on the trial-spawner/vault/cobweb family and
    /// nowhere else — the group vanilla wants heard late from across a room.
    #[test]
    fn only_the_trial_spawner_family_delays_by_distance() {
        let delayed: HashSet<i32> = SOUNDS
            .iter()
            .filter(|s| s.distance_delay)
            .map(|s| s.id)
            .collect();
        assert_eq!(
            delayed,
            HashSet::from([3012, 3013, 3014, 3015, 3016, 3018, 3019, 3020, 3021])
        );
    }

    /// Volume spans two orders of magnitude, which is the reason it is
    /// carried at all. Stated as a property so it survives a re-transcription.
    #[test]
    fn volume_is_not_uniformly_one() {
        assert_eq!(resolve(1025, 0, false).unwrap().volume, Some(0.05));
        assert_eq!(resolve(1016, 0, false).unwrap().volume, Some(10.0));
        assert_eq!(resolve(3001, 0, false).unwrap().volume, Some(64.0));
        let ones = SOUNDS.iter().filter(|s| s.volume == Some(1.0)).count();
        assert!(ones < SOUNDS.len(), "every row has volume 1.0");
    }

    /// The one row vanilla computes a volume for.
    #[test]
    fn only_the_sculk_charge_leaves_its_volume_unstated() {
        let unstated: Vec<i32> = SOUNDS
            .iter()
            .filter(|s| s.volume.is_none())
            .map(|s| s.id)
            .collect();
        assert_eq!(unstated, [3006]);
        assert!(resolve(3006, 0, false).unwrap().note.is_some());
    }

    /// Two ids share one `case` label and so one sound; two more are twins
    /// that differ only in particles. Pinned because a transcription that
    /// merged or dropped either would look tidy and be wrong.
    #[test]
    fn ids_that_share_a_sound_share_it_exactly() {
        assert_eq!(
            resolve(2002, 0, false).unwrap().sound,
            resolve(2007, 0, false).unwrap().sound
        );
        assert_eq!(
            resolve(3013, 0, false).unwrap().sound,
            resolve(3019, 0, false).unwrap().sound
        );
    }

    /// An unknown id is `None` rather than a default — the M64 rule.
    #[test]
    fn an_unknown_or_silent_or_derived_id_resolves_to_nothing() {
        assert!(resolve(9999, 0, false).is_none());
        assert!(resolve(0, 0, false).is_none());
        for &id in SILENT {
            assert!(resolve(id, 0, false).is_none(), "{id} is silent");
        }
        for d in DERIVED {
            assert!(resolve(d.id, 0, false).is_none(), "{} is derived", d.id);
            assert!(resolve(d.id, 1, false).is_none(), "{} is derived", d.id);
        }
    }

    /// `rows_for` sees every branch of a gated id, which `resolve` cannot.
    #[test]
    fn rows_for_enumerates_every_branch_of_a_gated_id() {
        assert_eq!(rows_for(1009).count(), 2);
        assert_eq!(rows_for(1500).count(), 2);
        assert_eq!(rows_for(3020).count(), 2);
        assert_eq!(rows_for(1000).count(), 1);
        assert_eq!(rows_for(9999).count(), 0);
    }

    /// Every named sound is a registered `sound_event`. This is the join the
    /// table exists to make, and it is checked against the real report rather
    /// than against another hand-written list.
    #[test]
    fn every_named_sound_is_in_the_real_sound_event_registry() {
        let Some(paths) = crate::DataPaths::for_version("26.2") else {
            eprintln!("SKIP: no config dir");
            return;
        };
        if !paths.registries_json().exists() {
            eprintln!("SKIP: no datagen report");
            return;
        }
        let registry =
            crate::sound_events::SoundEvents::load(&paths.registries_json()).expect("registry");
        let unknown: Vec<&str> = SOUNDS
            .iter()
            .map(|s| s.sound)
            .filter(|n| registry.id_of(n).is_none())
            .collect();
        assert!(unknown.is_empty(), "not registered sound events: {unknown:?}");
    }

    /// …and every one of them has a `sounds.json` entry that actually
    /// resolves to a file, which is the half a registry lookup cannot prove.
    #[test]
    fn every_named_sound_resolves_to_a_file_through_sounds_json() {
        use crate::sounds_json::{load_from_asset_store, shared_assets_dir};
        let Some(root) = shared_assets_dir() else {
            eprintln!("SKIP: no config dir");
            return;
        };
        if !root.join("indexes/32.json").exists() {
            eprintln!("SKIP: no asset store");
            return;
        }
        let idx = load_from_asset_store(&root, "32").expect("load");
        let mut silent = Vec::new();
        for s in SOUNDS {
            if idx.get_sound_seeded(s.sound, 0).is_none() {
                silent.push(s.sound);
            }
        }
        assert!(silent.is_empty(), "resolve to no file: {silent:?}");
    }
}
