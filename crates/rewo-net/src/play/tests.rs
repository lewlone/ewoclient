use super::*;

mod push_tests {
    use super::push_delta;

    /// Below vanilla's 0.01 threshold nothing happens (exactly-overlapping
    /// entities would otherwise divide by ~0).
    #[test]
    fn coincident_entities_do_not_push() {
        assert_eq!(push_delta(0.0, 0.0), (0.0, 0.0));
        assert_eq!(push_delta(0.005, -0.004), (0.0, 0.0));
    }

    /// Vanilla's math, computed by hand for a 1-block separation along +x:
    /// dd = sqrt(absMax(1,0)) = 1, pow = min(1, 1/1) = 1,
    /// so the impulse is 1/1 * 1 * 0.05 = 0.05 on x and 0 on z.
    #[test]
    fn unit_separation_matches_vanilla() {
        let (x, z) = push_delta(1.0, 0.0);
        assert!((x - 0.05).abs() < 1e-12, "x={x}");
        assert_eq!(z, 0.0);
    }

    /// The push is directional and symmetric under negation.
    #[test]
    fn push_is_antisymmetric() {
        let (ax, az) = push_delta(0.4, -0.3);
        let (bx, bz) = push_delta(-0.4, 0.3);
        assert!((ax + bx).abs() < 1e-12 && (az + bz).abs() < 1e-12);
        assert!(ax > 0.0 && az < 0.0, "points away along the separation");
    }

    /// Closer than one block, `pow = 1/dd > 1` is clamped to 1 — so the shove
    /// never exceeds PUSH_SPEED in magnitude per axis component.
    #[test]
    fn close_range_push_is_clamped() {
        let (x, z) = push_delta(0.05, 0.0);
        assert!(x <= 0.05 + 1e-12, "clamped, got {x}");
        assert!(x > 0.0 && z == 0.0);
    }
}

mod dirty_tests {
    use super::columns_seeing_block;

    #[test]
    fn an_interior_block_dirties_only_its_column() {
        assert_eq!(columns_seeing_block(5, 7), vec![(0, 0)]);
    }

    #[test]
    fn an_edge_block_dirties_the_neighbour_across_the_edge() {
        assert_eq!(columns_seeing_block(16, 7), vec![(1, 0), (0, 0)]);
        assert_eq!(columns_seeing_block(3, -1), vec![(0, -1), (0, 0)]);
    }

    #[test]
    fn a_corner_block_dirties_the_diagonal_too() {
        let mut got = columns_seeing_block(-16, -1);
        got.sort();
        assert_eq!(got, vec![(-2, -1), (-2, 0), (-1, -1), (-1, 0)]);
    }
}

mod section_update_tests {
    use super::*;

    /// Mirrors `SectionPos.asLong`, so the test states the encoding
    /// independently of the decoder under test.
    fn as_long(x: i64, y: i64, z: i64) -> u64 {
        (((x & 0x3F_FFFF) << 42) | (y & 0xF_FFFF) | ((z & 0x3F_FFFF) << 20)) as u64
    }

    #[test]
    fn section_pos_roundtrips_including_negatives() {
        for (x, y, z) in [
            (0, 0, 0),
            (1, 2, 3),
            (-1, -1, -1),
            (-3000, -4, 2999),
            (100, 19, -100),
        ] {
            assert_eq!(
                unpack_section_pos(as_long(x as i64, y as i64, z as i64)),
                (x, y, z),
                "section ({x},{y},{z})"
            );
        }
    }

    #[test]
    fn section_offset_uses_the_x_z_y_nibble_order() {
        // Vanilla packs `x << 8 | z << 4 | y`.
        for (x, y, z) in [(0, 0, 0), (15, 15, 15), (1, 2, 3), (9, 4, 7)] {
            let packed = (x << 8) | (z << 4) | y;
            assert_eq!(
                unpack_section_offset(packed),
                (x, y, z),
                "offset ({x},{y},{z})"
            );
        }
    }

    #[test]
    fn a_change_entry_splits_into_state_and_position() {
        // Wire form: `stateId << 12 | posInSection`.
        let packed: u64 = (1234u64 << 12) | ((5 << 8) | (6 << 4) | 7);
        assert_eq!(packed >> 12, 1234);
        assert_eq!(unpack_section_offset((packed & 4095) as i32), (5, 7, 6));
    }
}

mod login_dimension_tests {
    use super::*;
    use crate::dimension_parse::builtin as fx;
    use crate::spawn_info::GlobalPos;

    /// A registry in a **deliberately non name-sorted** wire order: the Nether
    /// is holder 0 and the Overworld is holder 2. Any name-keyed shortcut in the
    /// selection path fails immediately here.
    fn registry() -> Vec<DimensionTypeDef> {
        crate::dimension_parse::parse_dimension_registry_packet(&fx::registry_packet(&[
            ("minecraft:the_nether", fx::the_nether()),
            ("minecraft:the_end", fx::the_end()),
            ("minecraft:overworld", fx::overworld()),
        ]))
        .expect("fixture registry must parse")
        .expect("packet is the dimension_type registry")
    }

    /// A spawn info naming `level` on dimension-type holder `holder`, with every
    /// other field filled in so nothing about the case under test depends on a
    /// default.
    fn spawn(holder: i32, level: &str) -> CommonPlayerSpawnInfo {
        CommonPlayerSpawnInfo {
            dimension_type: holder,
            dimension: level.into(),
            seed: 0x0bad_f00d_dead_beefu64 as i64,
            game_type: 1,
            previous_game_type: Some(0),
            is_debug: false,
            is_flat: false,
            last_death_location: Some(GlobalPos {
                dimension: "minecraft:overworld".into(),
                x: -3,
                y: -59,
                z: 7,
            }),
            portal_cooldown: 0,
            sea_level: 63,
        }
    }

    /// The pre-login world is a plain Overworld placeholder, so a test that
    /// lands on the Nether cannot pass by accident.
    fn placeholder_world() -> World {
        let world = World::new(DimensionShape::OVERWORLD);
        assert_eq!(world.shape, DimensionShape::OVERWORLD);
        assert!(world.has_sky_light());
        world
    }

    /// Raw holder 0 is the **first synced entry**, never "inline" and never a
    /// default: here that entry is the Nether, so the resolved shape is 0..256
    /// with no skylight rather than the placeholder's -64..320 with skylight.
    #[test]
    fn raw_holder_zero_selects_the_first_entry_even_when_it_is_the_nether() {
        let defs = registry();
        assert_eq!(defs[0].name, "minecraft:the_nether", "fixture precondition");
        let mut world = placeholder_world();
        let active = apply_spawn_info(&mut world, &defs, &spawn(0, "minecraft:the_nether"));

        assert_eq!(active.holder, 0);
        assert_eq!(active.def.name, "minecraft:the_nether");
        assert_eq!(active.def.shape, DimensionShape::NETHER);
        assert!(!active.def.has_sky_light);
        assert_eq!(active.def.skybox, Skybox::None);
        // …and the world is now decoding chunks against exactly that.
        assert_eq!(world.shape, DimensionShape::NETHER);
        assert_ne!(world.shape, DimensionShape::OVERWORLD);
        assert!(!world.has_sky_light());
        assert_eq!(
            world.cardinal_light_type(),
            rewo_world::dimension::CardinalLightType::Nether
        );
    }

    /// The active level key is `CommonPlayerSpawnInfo.dimension`, NOT the
    /// selected dimension **type**'s registry name. A datapack level built on
    /// the vanilla overworld type shares that type's name with
    /// `minecraft:overworld` — reading the key off `def.name` would report the
    /// wrong world for every such level, and would be indistinguishable from
    /// correct on a vanilla-only server.
    #[test]
    fn active_level_key_comes_from_the_spawn_info_not_the_type_name() {
        let defs = registry();
        let mut world = placeholder_world();
        let active = apply_spawn_info(&mut world, &defs, &spawn(2, "rewo:mining_world"));

        assert_eq!(active.key, "rewo:mining_world");
        assert_eq!(active.def.name, "minecraft:overworld");
        assert_ne!(
            active.key, active.def.name,
            "key is the level, not the type"
        );
        // The type still resolved normally — the two identifiers are separate,
        // not alternatives.
        assert_eq!(active.holder, 2);
        assert_eq!(world.shape, DimensionShape::OVERWORLD);
        assert!(world.has_sky_light());
    }

    /// A holder the synced registry does not contain degrades to the *named*
    /// unresolved fallback, and the packet's own level key and holder id survive
    /// verbatim — the diagnostic must still say which world the server claimed.
    #[test]
    fn an_unresolved_holder_keeps_the_packets_key_and_holder() {
        let defs = registry();
        let mut world = placeholder_world();
        let active = apply_spawn_info(&mut world, &defs, &spawn(99, "rewo:mining_world"));

        assert_eq!(active.key, "rewo:mining_world");
        assert_eq!(active.holder, 99);
        assert_eq!(active.def.name, "rewo:unresolved_dimension_type/99");
        assert!(
            defs.iter().all(|d| d.name != active.def.name),
            "the fallback must never claim to be a synced entry"
        );
        assert_eq!(active.def.shape, DimensionShape::OVERWORLD);
    }
}

mod respawn_tests {
    use super::*;
    use crate::dimension_parse::builtin as fx;
    use crate::spawn_info::GlobalPos;
    use rewo_world::entities::EntityState;

    /// The same deliberately non name-sorted registry the login tests use:
    /// Nether 0, the_end 1, Overworld 2. Any name-keyed shortcut fails here.
    fn registry() -> Vec<DimensionTypeDef> {
        crate::dimension_parse::parse_dimension_registry_packet(&fx::registry_packet(&[
            ("minecraft:the_nether", fx::the_nether()),
            ("minecraft:the_end", fx::the_end()),
            ("minecraft:overworld", fx::overworld()),
        ]))
        .expect("fixture registry must parse")
        .expect("packet is the dimension_type registry")
    }

    const NETHER_HOLDER: i32 = 0;
    const OVERWORLD_HOLDER: i32 = 2;

    fn spawn(holder: i32, level: &str, seed: i64) -> CommonPlayerSpawnInfo {
        CommonPlayerSpawnInfo {
            dimension_type: holder,
            dimension: level.into(),
            seed,
            game_type: 1,
            previous_game_type: Some(0),
            is_debug: false,
            is_flat: false,
            last_death_location: Some(GlobalPos {
                dimension: "minecraft:overworld".into(),
                x: -3,
                y: -59,
                z: 7,
            }),
            portal_cooldown: 0,
            sea_level: 63,
        }
    }

    /// The world-side session state as plain locals: a `PlaySession` owns a
    /// socket and cannot be built in a test, but [`WorldTransition`] borrows
    /// exactly these fields and nothing else.
    struct Harness {
        world: World,
        dirty: std::collections::HashSet<(i32, i32)>,
        removed: Vec<(i32, i32)>,
        light: rewo_world::light::LightEngine,
        day_ticks: Option<i64>,
        clocks: ClockManager,
        game_time: Option<i64>,
        end_flash: Option<rewo_world::end_flash::EndFlashState>,
        weather: rewo_world::weather::WeatherState,
        border: rewo_world::border::WorldBorder,
        biome_zoom_seed: Option<i64>,
        sea_level: Option<i32>,
        colormaps: rewo_world::biome::Colormaps,
        key: Option<String>,
        holder: Option<i32>,
        ty: Option<DimensionTypeDef>,
        generation: u64,
        transitions: Vec<DimensionTransition>,
    }

    /// A live-looking Overworld session: **three** loaded columns (two of them
    /// also queued for re-mesh), an entity, a running world clock, and a
    /// generation already past 0 so an increment can't be confused with a reset.
    const OLD_COLUMNS: [(i32, i32); 3] = [(0, 0), (1, -2), (-3, 5)];

    fn overworld_session(defs: &[DimensionTypeDef], generation: u64) -> Harness {
        let mut world = World::for_dimension(&defs[OVERWORLD_HOLDER as usize]);
        for (cx, cz) in OLD_COLUMNS {
            world.ensure_column(cx, cz);
        }
        world
            .entities
            .add(7, EntityState::new(1, 42, 8.0, 70.0, 8.0, 0.0, 0.0));
        assert_eq!(world.loaded_columns(), 3, "precondition");
        assert!(world.has_sky_light(), "precondition");
        Harness {
            world,
            dirty: [(0, 0), (1, -2)].into_iter().collect(),
            removed: Vec::new(),
            light: rewo_world::light::LightEngine::new(),
            day_ticks: Some(189_121),
            clocks: {
                let mut m = ClockManager::default();
                m.last_game_time = 138_341;
                m.clocks
                    .push((0, WorldClock::from_state(138_341, 189_121, 0.25, 1.0)));
                m
            },
            game_time: Some(138_341),
            // Deliberately a live flash on a dimension that has none: a
            // transition that failed to rebuild it from the NEW dimension
            // type would otherwise be invisible against a `None` harness.
            end_flash: Some(rewo_world::end_flash::EndFlashState::default()),
            // Deliberately a storm: a transition that failed to clear it would
            // otherwise be invisible against a default-clear harness.
            weather: {
                let mut w = rewo_world::weather::WeatherState::default();
                w.set_rain(0.8);
                w.set_thunder(0.5);
                w
            },
            // Same argument as the storm: a small off-centre border, so a
            // transition that failed to reset it would not hide behind the
            // default's own numbers.
            border: {
                let mut b = rewo_world::border::WorldBorder::default();
                b.set_center(120.0, -64.0);
                b.set_size(500.0);
                b.set_warning_blocks(11);
                b
            },
            biome_zoom_seed: Some(0x0bad_f00d),
            sea_level: Some(63),
            colormaps: rewo_world::biome::Colormaps::neutral(),
            key: Some("minecraft:overworld".into()),
            holder: Some(OVERWORLD_HOLDER),
            ty: Some(defs[OVERWORLD_HOLDER as usize].clone()),
            generation,
            transitions: Vec::new(),
        }
    }

    impl Harness {
        fn respawn(&mut self, defs: &[DimensionTypeDef], spawn: &CommonPlayerSpawnInfo) -> bool {
            WorldTransition {
                world: &mut self.world,
                dirty: &mut self.dirty,
                removed: &mut self.removed,
                light: &mut self.light,
                day_ticks: &mut self.day_ticks,
                clocks: &mut self.clocks,
                game_time: &mut self.game_time,
                weather: &mut self.weather,
                border: &mut self.border,
                biome_zoom_seed: &mut self.biome_zoom_seed,
                sea_level: &mut self.sea_level,
                biome_registry: None,
                colormaps: &self.colormaps,
                active_key: &mut self.key,
                active_holder: &mut self.holder,
                active_type: &mut self.ty,
                end_flash: &mut self.end_flash,
                generation: &mut self.generation,
                transitions: &mut self.transitions,
            }
            .apply_respawn(defs, spawn)
        }
    }

    /// Overworld → Nether: every old column is queued for the renderer to free,
    /// the world is a *fresh* Nether (0..256, no sky light, no columns, no
    /// entities), and the per-level clock/dirty/light state is back to its
    /// pre-`set_time` values.
    #[test]
    fn a_changed_key_rebuilds_the_world_and_queues_every_old_column() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", -1)));

        // Every old coordinate reached `removed` — exactly once, and only those.
        let mut removed = s.removed.clone();
        removed.sort_unstable();
        let mut expected = OLD_COLUMNS;
        expected.sort_unstable();
        assert_eq!(removed, expected, "the renderer must free all three");

        // A fresh Nether, not a re-pointed Overworld.
        assert_eq!(s.world.shape, DimensionShape::NETHER);
        assert_ne!(s.world.shape, DimensionShape::OVERWORLD);
        assert!(!s.world.has_sky_light());
        assert_eq!(s.world.loaded_columns(), 0, "old columns are gone");
        assert_eq!(s.world.entities.len(), 0, "entities go with the old world");

        // Light-relevant state: the lighting contract is the Nether's, so an
        // unloaded read is dark rather than the Overworld's impossible sky 15 —
        // and the fresh engine has nothing queued from the old shape.
        assert_eq!(s.world.light_at(0, 70, 0), (0, 0));
        assert_eq!(s.world.brightness_at(0, 70, 0), 0);
        let tables = rewo_world::light::LightTables {
            emission: &[],
            dampening: &[],
            face_occludes: &[],
        };
        assert!(
            s.light
                .on_block_change(&mut s.world, tables, 0, 70, 0, 0, 0)
                .is_empty(),
            "a fresh engine touches nothing in an empty world"
        );

        // Per-level state cleared.
        assert!(s.dirty.is_empty(), "no stale column is queued for re-mesh");
        assert_eq!(s.day_ticks, None);
        assert!(s.clocks.is_empty(), "every clock, not only the overworld's");
        // `ClientLevel.java:255` builds this from the NEW dimension type, and
        // the Nether's skybox is `none` — so a level change destroys the
        // flash rather than carrying it. The harness seeds a live one on
        // purpose, or a transition that simply never touched the field would
        // pass.
        assert!(
            s.end_flash.is_none(),
            "the Nether has no end flashes, whatever the old level had"
        );
        assert_eq!(s.game_time, None);

        // The new dimension, and the seed the biome layer fiddles with.
        assert_eq!(s.key.as_deref(), Some("minecraft:the_nether"));
        assert_eq!(s.holder, Some(NETHER_HOLDER));
        assert_eq!(
            s.ty.as_ref().map(|d| d.name.as_str()),
            Some("minecraft:the_nether")
        );
        assert_eq!(s.biome_zoom_seed, Some(-1));

        // Generation incremented by exactly one, and the history is exact.
        assert_eq!(s.generation, 5);
        assert_eq!(
            s.transitions,
            vec![DimensionTransition {
                old_key: Some("minecraft:overworld".into()),
                new_key: "minecraft:the_nether".into(),
                holder: NETHER_HOLDER,
                type_name: "minecraft:the_nether".into(),
                shape: DimensionShape::NETHER,
                has_sky_light: false,
                skybox: Skybox::None,
                ambient_light: 0.1,
                cardinal_light_type: CardinalLightType::Nether,
                has_day_timeline: false,
                generation: 5,
                // The witnesses: three loaded columns left, three queued for the
                // renderer to free, none carried into the replacement, no stale
                // re-mesh entry, and the whole clock back to pre-`set_time`.
                old_columns: 3,
                queued_for_removal: 3,
                removal_queue_len: 3,
                new_world_columns: 0,
                dirty_after: 0,
                clock_reset: true,
            }]
        );
    }

    /// The discard witnesses are *measurements*, not constants: with a
    /// pre-loaded removal queue the transition still reports exactly what it
    /// pushed, and the queue length grows to hold both.
    ///
    /// This is the property no observer outside the transition can check —
    /// coordinates cannot prove it, because the Nether loads column (0,0) too.
    #[test]
    fn the_discard_witnesses_count_this_transitions_own_push() {
        let defs = registry();
        let mut s = overworld_session(&defs, 0);
        // A column the app has not drained yet, from an earlier unload.
        s.removed.push((99, 99));
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 7)));
        let t = &s.transitions[0];
        assert_eq!(t.old_columns, 3, "the world we left had three columns");
        assert_eq!(t.queued_for_removal, 3, "this transition pushed three");
        assert_eq!(t.removal_queue_len, 4, "the pre-existing entry is still queued");
        assert_eq!(t.new_world_columns, 0);
        assert_eq!(t.dirty_after, 0);
        assert!(t.clock_reset);
        assert_eq!(t.type_name, "minecraft:the_nether");
        assert_eq!(t.cardinal_light_type, CardinalLightType::Nether);
        assert_eq!(t.ambient_light, 0.1);
        assert!(!t.has_day_timeline);
        assert_eq!(s.removed.len(), 4);
    }

    /// A respawn naming the level we are already in (the ordinary death
    /// respawn): the world, its columns, the clock, the generation and the
    /// history all survive untouched — and so do the dimension type and the
    /// seed, because vanilla builds no new `ClientLevel` to apply them to.
    #[test]
    fn a_same_key_respawn_retains_the_world_generation_and_history() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        let digest = s.world.digest();
        // A same-key packet that nonetheless names a *different* holder and
        // seed: neither may be applied behind the retained chunks.
        assert!(!s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:overworld", 999)));

        assert_eq!(s.world.loaded_columns(), 3, "columns retained");
        assert_eq!(s.world.digest(), digest, "the world is untouched");
        assert_eq!(s.world.shape, DimensionShape::OVERWORLD);
        assert!(s.world.has_sky_light());
        assert_eq!(s.world.entities.len(), 1, "entities retained");
        assert!(s.removed.is_empty(), "nothing to free");
        assert_eq!(s.dirty.len(), 2, "re-mesh queue retained");

        assert_eq!(s.day_ticks, Some(189_121));
        assert_eq!(s.game_time, Some(138_341));
        assert_eq!(s.clocks.peek(0).unwrap().total, 189_121);

        assert_eq!(s.holder, Some(OVERWORLD_HOLDER), "type not re-applied");
        assert_eq!(
            s.ty.as_ref().map(|d| d.name.as_str()),
            Some("minecraft:overworld")
        );
        assert_eq!(s.biome_zoom_seed, Some(0x0bad_f00d), "seed retained");
        assert_eq!(s.generation, 4, "not a transition");
        assert!(s.transitions.is_empty(), "history unmoved");
        assert_eq!(
            s.weather.rain_level(),
            0.8,
            "no new level, so the storm keeps falling"
        );
        assert_eq!(s.border.size(), 500.0, "and the same border still stands");
    }

    /// Weather is `ClientLevel` state: a real dimension change discards it, so
    /// walking into the Nether cannot carry the Overworld's storm along. The
    /// harness starts at rain 0.8 / thunder 0.5 so this can fail.
    #[test]
    fn a_dimension_change_clears_the_weather() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        assert_eq!(s.weather.rain_level(), 0.8, "precondition");
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 7)));
        assert_eq!(s.weather.rain_level(), 0.0);
        assert_eq!(s.weather.thunder_level(), 0.0);
    }

    /// The border is `ClientLevel` state on the same argument (M80). The
    /// harness starts at a 500-block border centred on (120, -64) so a
    /// transition that carried it through would be visible in all three of
    /// size, centre and warning distance.
    #[test]
    fn a_dimension_change_clears_the_world_border() {
        let defs = registry();
        let mut s = overworld_session(&defs, 4);
        assert_eq!(s.border.size(), 500.0, "precondition");
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 7)));
        assert_eq!(s.border.size(), rewo_world::border::MAX_SIZE);
        assert_eq!(s.border.center_x(), 0.0);
        assert_eq!(s.border.center_z(), 0.0);
        assert_eq!(s.border.warning_blocks(), 5);
    }

    /// The generation is a counter, not an index: at `u64::MAX` it wraps to 0
    /// rather than panicking on overflow, and the recorded transition carries
    /// the wrapped value.
    #[test]
    fn the_generation_wraps_at_u64_max() {
        let defs = registry();
        let mut s = overworld_session(&defs, u64::MAX);
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 0)));
        assert_eq!(s.generation, 0);
        assert_eq!(s.transitions[0].generation, 0);
    }

    /// Two changes in a row: the history is append-only and oldest-first, and
    /// the second transition's `old_key` is the first's `new_key`.
    #[test]
    fn successive_changes_append_an_oldest_first_history() {
        let defs = registry();
        let mut s = overworld_session(&defs, 0);
        assert!(s.respawn(&defs, &spawn(NETHER_HOLDER, "minecraft:the_nether", 1)));
        assert!(s.respawn(&defs, &spawn(OVERWORLD_HOLDER, "rewo:mining_world", 2)));

        assert_eq!(s.generation, 2);
        assert_eq!(s.transitions.len(), 2);
        assert_eq!(s.transitions[0].new_key, "minecraft:the_nether");
        assert_eq!(
            s.transitions[1].old_key.as_deref(),
            Some("minecraft:the_nether")
        );
        assert_eq!(s.transitions[1].new_key, "rewo:mining_world");
        // The level key is the packet's, the type is the holder's — a datapack
        // level on the vanilla overworld type keeps both straight.
        assert_eq!(s.key.as_deref(), Some("rewo:mining_world"));
        assert_eq!(
            s.ty.as_ref().map(|d| d.name.as_str()),
            Some("minecraft:overworld")
        );
        assert_eq!(s.world.shape, DimensionShape::OVERWORLD);
        assert!(s.world.has_sky_light());
    }

    // -- the camera (M162) -------------------------------------------------

    /// **`None` until the server has positioned us**, which is Rewo's
    /// `camera.isInitialized()`.
    ///
    /// `PlaySession::spawned` goes true on the first `player_position`
    /// teleport; before it, `self.player` is at whatever the constructor left,
    /// which is the origin. Handing that out as a camera puts a wither's roar
    /// two blocks from `(0, 0, 0)` for anyone who spawns anywhere else — and
    /// `LevelEventHandler.java:66` has no `else`, so vanilla plays nothing at
    /// all in that state.
    #[test]
    fn the_camera_is_absent_until_the_server_positions_us() {
        let p = moving_player();
        assert_eq!(PlaySession::camera_eye(&p, false), None);
        assert!(PlaySession::camera_eye(&p, true).is_some());
    }

    /// It is the EYE, not the feet.
    ///
    /// 1.62 blocks is a quarter of the bearing's own 2.0-block radius, so a
    /// feet reading is not a rounding difference — it tilts every global
    /// event's direction noticeably downward, and nothing about the sound's
    /// distance would look wrong.
    #[test]
    fn the_camera_is_the_eye_and_not_the_feet() {
        let p = moving_player();
        let got = PlaySession::camera_eye(&p, true).expect("spawned");
        assert_eq!(got, [p.x, p.y + rewo_world::physics::EYE_HEIGHT, p.z]);
        assert_ne!(got[1], p.y, "the feet would be 1.62 blocks low");
        // x and z pass through untouched — a transposed pair would put the
        // listener somewhere else entirely.
        assert_eq!((got[0], got[2]), (p.x, p.z));
        assert_ne!(got[0], got[2], "the fixture must not hide a transposition");
    }

    // -- the local player --------------------------------------------------

    /// A player mid-flight, so nothing below can pass by starting at a default.
    fn moving_player() -> PlayerState {
        PlayerState {
            vx: 0.25,
            vy: -0.6,
            vz: -0.125,
            yaw: 137.5,
            pitch: -22.5,
            on_ground: true,
            horizontal_collision: true,
            ..PlayerState::at(120.5, 71.0, -33.25)
        }
    }

    struct PlayerHarness {
        player: PlayerState,
        health: f32,
        food: i32,
        dead: bool,
        spawned: bool,
        last_pos: (f64, f64, f64),
        last_rot: (f32, f32),
        reminder: u32,
        last_on_ground: bool,
        last_horiz: bool,
        last_input_flags: u8,
        local_fall_flying: bool,
    }

    fn player_harness() -> PlayerHarness {
        PlayerHarness {
            player: moving_player(),
            health: 3.5,
            food: 6,
            dead: true,
            spawned: true,
            last_pos: (120.5, 71.0, -33.25),
            last_rot: (137.5, -22.5),
            reminder: 13,
            last_on_ground: true,
            last_horiz: true,
            last_input_flags: 0b0101_0001,
            local_fall_flying: true,
        }
    }

    impl PlayerHarness {
        fn respawn(&mut self, keep_entity_data: bool) {
            LocalPlayerRespawn {
                player: &mut self.player,
                health: &mut self.health,
                food: &mut self.food,
                dead: &mut self.dead,
                spawned: &mut self.spawned,
                last_pos: &mut self.last_pos,
                last_rot: &mut self.last_rot,
                reminder: &mut self.reminder,
                last_on_ground: &mut self.last_on_ground,
                last_horiz: &mut self.last_horiz,
                last_input_flags: &mut self.last_input_flags,
                local_fall_flying: &mut self.local_fall_flying,
            }
            .apply(keep_entity_data);
        }
    }

    /// `dataToKeep` bit 2 (`shouldKeep((byte)2)`): the statements Rewo can
    /// represent — `setDeltaMovement(old)`, `setYRot(old)`, `setXRot(old)`, and
    /// the `assignValues` health entry — carry over bit-exactly, and the
    /// constructor's `lastSentInput` is the old player's rather than
    /// `Input.EMPTY`.
    #[test]
    fn keeping_entity_data_preserves_velocity_and_both_rotations() {
        let mut h = player_harness();
        h.respawn(true);

        let old = moving_player();
        assert_eq!(
            (h.player.vx, h.player.vy, h.player.vz),
            (old.vx, old.vy, old.vz)
        );
        assert_eq!(h.player.yaw, old.yaw);
        assert_eq!(h.player.pitch, old.pitch);
        assert_eq!(h.last_input_flags, 0b0101_0001, "old lastSentInput kept");

        // Everything else is still the fresh entity's — bit 2 keeps entity
        // *data*, not the entity's position or its send-cadence bookkeeping.
        assert_eq!((h.player.x, h.player.y, h.player.z), (0.0, 0.0, 0.0));
        assert!(!h.player.on_ground && !h.player.horizontal_collision);
        assert_eq!(h.last_pos, (0.0, 0.0, 0.0));
        assert_eq!(h.last_rot, (0.0, 0.0));
        assert_eq!(h.reminder, 0);
        assert!(!h.last_on_ground && !h.last_horiz);
        // Health is `SynchedEntityData`, so bit 2 preserves the old player's
        // non-default value exactly; food is `FoodData` and is always fresh.
        assert_eq!(h.health, 3.5, "DATA_HEALTH_ID carried by assignValues");
        assert_eq!(h.food, 20, "FoodData is not synched data — always fresh");
        // `DATA_SHARED_FLAGS_ID` travels on the same `assignValues`, so a
        // player who was mid-glide when they died comes back mid-glide.
        assert!(h.local_fall_flying, "FLAG_FALL_FLYING carried by assignValues");
        assert!(!h.dead);
        assert!(
            !h.spawned,
            "not a live participant until the teleport lands"
        );
    }

    /// `dataToKeep` 0: `resetPos()` zeroes the delta movement and the X rotation,
    /// `handleRespawn` then sets the Y rotation to -180, and the constructor
    /// supplies everything else — including `Input.EMPTY`.
    #[test]
    fn without_keep_the_player_is_the_freshly_constructed_one() {
        let mut h = player_harness();
        h.respawn(false);

        assert_eq!((h.player.vx, h.player.vy, h.player.vz), (0.0, 0.0, 0.0));
        assert_eq!(h.player.pitch, 0.0, "resetPos setXRot(0)");
        assert_eq!(h.player.yaw, -180.0, "handleRespawn setYRot(-180)");
        assert_eq!(h.last_input_flags, 0, "Input.EMPTY");
        assert_eq!((h.player.x, h.player.y, h.player.z), (0.0, 0.0, 0.0));
        assert!(!h.player.on_ground && !h.player.horizontal_collision);
        assert_eq!(h.last_pos, (0.0, 0.0, 0.0));
        assert_eq!(h.last_rot, (0.0, 0.0));
        assert_eq!(h.reminder, 0);
        assert!(!h.last_on_ground && !h.last_horiz);
        // No bit 2 → no `assignValues`, so the harness's non-default 3.5 health
        // is gone and the fresh player's 20 stands.
        assert_eq!((h.health, h.food), (20.0, 20));
        // The fresh entity's shared flags are the defaults, and the server will
        // never re-state one that is already default — so without this reset a
        // glide outlives its player.
        assert!(!h.local_fall_flying, "fresh entity starts with the flag low");
        assert!(!h.dead);
        assert!(!h.spawned);
    }

    /// Bit 1 (`KEEP_ATTRIBUTE_MODIFIERS`) selects `assignAllValues` vs
    /// `assignBaseValues` on an `AttributeMap` Rewo does not model, so it is a
    /// documented no-op: the two masks that differ only in bit 1 must land on
    /// identical player state.
    #[test]
    fn the_attribute_modifier_bit_is_a_no_op() {
        for (a, b) in [
            (0u8, RespawnInfo::KEEP_ATTRIBUTE_MODIFIERS),
            (RespawnInfo::KEEP_ENTITY_DATA, RespawnInfo::KEEP_ALL_DATA),
        ] {
            let keeps_entity_data = |m: u8| m & RespawnInfo::KEEP_ENTITY_DATA != 0;
            let mut x = player_harness();
            x.respawn(keeps_entity_data(a));
            let mut y = player_harness();
            y.respawn(keeps_entity_data(b));
            assert_eq!(
                (
                    x.player.vx,
                    x.player.vy,
                    x.player.vz,
                    x.player.yaw,
                    x.player.pitch
                ),
                (
                    y.player.vx,
                    y.player.vy,
                    y.player.vz,
                    y.player.yaw,
                    y.player.pitch
                ),
                "dataToKeep {a} and {b} differ only in the attribute bit"
            );
            assert_eq!(x.last_input_flags, y.last_input_flags);
            assert_eq!(
                (x.health, x.food, x.dead, x.spawned, x.local_fall_flying),
                (y.health, y.food, y.dead, y.spawned, y.local_fall_flying)
            );
        }
    }
}

/// The map half of M149c — what `clock_tests`' harness does for itself and so
/// cannot witness.
mod clock_map_tests {
    use super::{ClockManager, WorldClock};

    /// A vanilla server sends the Overworld's clock **and** the End's in one
    /// `set_time`. M12 kept the first and dropped the second; both are held
    /// now, independently.
    #[test]
    fn two_clocks_are_held_and_advance_independently() {
        let mut m = ClockManager::default();
        m.handle_updates(1000, &[(0, 5_000, 0.0, 1.0), (1, 77, 0.0, 0.5)]);
        assert_eq!(m.peek(0).unwrap().total, 5_000);
        assert_eq!(m.peek(1).unwrap().total, 77);

        // One shared delta, each clock scaling it by its own rate.
        m.handle_updates(1100, &[]);
        assert_eq!(m.peek(0).unwrap().total, 5_100, "rate 1 takes the whole delta");
        assert_eq!(m.peek(1).unwrap().total, 127, "rate 0.5 takes half of it");
    }

    /// `getTotalTicks` is `computeIfAbsent` (`ClientClockManager.java:15-17,
    /// 41-43`), so **asking creates**. The created instance is not inert: it
    /// carries `rate = 1.0` and counts up from the next tick.
    ///
    /// Returning 0 without creating looks identical on the tick you ask and
    /// diverges forever after — and for the End flash it is the difference
    /// between a schedule that runs and one pinned in interval 0, where
    /// `EndFlashState` never flashes.
    #[test]
    fn reading_an_unsent_clock_creates_it_and_it_then_advances() {
        let mut m = ClockManager::default();
        m.handle_updates(1000, &[(0, 5_000, 0.0, 1.0)]);
        assert!(m.peek(7).is_none(), "not there before it is asked for");

        assert_eq!(m.total_ticks(7), 0, "a fresh instance reads zero");
        assert!(m.peek(7).is_some(), "and asking is what created it");

        m.handle_updates(1050, &[]);
        assert_eq!(m.peek(7).unwrap().total, 50, "rate 1.0, so it counts up");
        assert_eq!(m.peek(0).unwrap().total, 5_050, "and the other is unaffected");
    }

    /// `lastTickGameTime` is the **manager's**, not the instance's
    /// (`ClientClockManager.java:19-21`): a clock minted between two ticks
    /// receives the whole delta on the next one, not just the part after it
    /// appeared.
    #[test]
    fn a_minted_clock_inherits_the_managers_last_tick_time() {
        let mut m = ClockManager::default();
        m.handle_updates(1000, &[(0, 0, 0.0, 1.0)]);
        // Minted at "1000" as far as the manager is concerned.
        assert_eq!(m.total_ticks(3), 0);
        m.handle_updates(1020, &[]);
        assert_eq!(
            m.peek(3).unwrap().total,
            20,
            "the whole 20-tick delta, because the manager's clock is shared"
        );
    }

    /// The tick-then-overwrite order, at the map level: an entry in the same
    /// packet lands on the explicit value, not the advanced one.
    #[test]
    fn an_explicit_entry_overwrites_the_advance_it_shares_a_packet_with() {
        let mut m = ClockManager::default();
        m.handle_updates(100, &[(0, 5_000, 0.0, 1.0)]);
        m.handle_updates(120, &[(0, 0, 0.0, 1.0)]);
        assert_eq!(m.peek(0).unwrap().total, 0, "/time set beats the +20 advance");
    }

    /// `clear` is the level boundary — every clock, not only the overworld's,
    /// and the shared last-tick time with them.
    #[test]
    fn clear_drops_every_clock_and_the_shared_time() {
        let mut m = ClockManager::default();
        m.handle_updates(1000, &[(0, 1, 0.0, 1.0), (1, 2, 0.0, 1.0)]);
        m.clear();
        assert!(m.is_empty());
        assert_eq!(m.last_game_time, 0);
        // And a clock minted after the clear starts from the new baseline
        // rather than inheriting the old level's game time.
        m.tick(50);
        assert_eq!(m.total_ticks(0), 0);
        m.tick(60);
        assert_eq!(m.peek(0).unwrap().total, 10);
    }

    /// `peek` must not mint — it is M12's day-tick fallback's only caller, and
    /// that fallback is defined by the *absence* of a clock.
    #[test]
    fn peek_does_not_mint() {
        let mut m = ClockManager::default();
        assert!(m.peek(0).is_none());
        assert!(m.is_empty(), "peeking created nothing");
        let _ = m.total_ticks(0);
        assert!(!m.is_empty(), "and total_ticks is what does create");
    }

    /// `ClientLevel.java:255`'s ternary, over the four vanilla skyboxes.
    ///
    /// The battery's argument for this existing: a mutation giving **every**
    /// dimension a flash survived the whole suite, because the only witness
    /// that could see it drove the respawn transition and the *login* path
    /// builds its level somewhere no test can reach.
    #[test]
    fn only_an_end_skybox_gets_a_flash() {
        use rewo_world::dimension::{DimensionTypeDef, Skybox};
        let with = |s: Skybox| {
            let mut d = DimensionTypeDef::unresolved_holder(0);
            d.skybox = s;
            super::end_flash_for_dimension(&d).is_some()
        };
        assert!(with(Skybox::End), "the End");
        assert!(!with(Skybox::Overworld), "the Overworld and its caves");
        assert!(!with(Skybox::None), "the Nether");
        // And a fresh one really is fresh — `EndFlashState`'s zeroed default
        // is what makes its first interval silent (M149a), so a flash handed
        // a used state would flash immediately on arrival.
        let mut d = DimensionTypeDef::unresolved_holder(0);
        d.skybox = Skybox::End;
        assert_eq!(
            super::end_flash_for_dimension(&d),
            Some(rewo_world::end_flash::EndFlashState::default())
        );
    }

    /// The three outcomes of `getDefaultClockTime`, which look like one
    /// number and are not.
    ///
    /// A dimension declaring **no** clock and one naming an **unknown** clock
    /// both read 0 forever; a dimension naming a real clock the server has
    /// never sent reads 0 *once* and then counts. Collapsing any two of them
    /// is invisible on the tick you look and permanent afterwards.
    #[test]
    fn default_clock_time_has_three_outcomes() {
        use rewo_world::dimension::{DimensionTypeDef, Skybox};

        let ids = vec!["minecraft:overworld".to_string(), "minecraft:the_end".to_string()];
        let dim = |clock: Option<&str>| {
            let mut d = DimensionTypeDef::unresolved_holder(0);
            d.skybox = Skybox::End;
            d.default_clock = clock.map(str::to_string);
            d
        };

        // 1. No clock declared — the Nether's case. Permanent zero.
        let mut m = ClockManager::default();
        m.handle_updates(1000, &[(0, 5_000, 0.0, 1.0)]);
        let nether = dim(None);
        assert_eq!(super::default_clock_time(Some(&nether), &ids, &mut m), 0);
        m.handle_updates(2000, &[]);
        assert_eq!(
            super::default_clock_time(Some(&nether), &ids, &mut m),
            0,
            "still zero a thousand ticks later"
        );
        assert_eq!(m.clocks.len(), 1, "and nothing was minted for it");

        // 2. Named but absent from the registry. Also permanently zero.
        let bogus = dim(Some("modded:elsewhere"));
        assert_eq!(super::default_clock_time(Some(&bogus), &ids, &mut m), 0);
        m.handle_updates(3000, &[]);
        assert_eq!(super::default_clock_time(Some(&bogus), &ids, &mut m), 0);
        assert_eq!(m.clocks.len(), 1, "an unknown name mints nothing");

        // 3. Named, real, never sent. Zero once, then counting.
        let end = dim(Some("minecraft:the_end"));
        assert_eq!(
            super::default_clock_time(Some(&end), &ids, &mut m),
            0,
            "the first read is what creates it"
        );
        m.handle_updates(3100, &[]);
        assert_eq!(
            super::default_clock_time(Some(&end), &ids, &mut m),
            100,
            "and from there it advances like any other clock"
        );

        // And the id really is the registry position, not a guess: the End's
        // clock is index 1, so a server state for holder 1 lands on it.
        m.handle_updates(3100, &[(1, 42_000, 0.0, 1.0)]);
        assert_eq!(super::default_clock_time(Some(&end), &ids, &mut m), 42_000);
        assert_eq!(
            super::default_clock_time(None, &ids, &mut m),
            0,
            "no active dimension at all is the same permanent zero"
        );
    }

    /// The `WorldClock` fields a minted instance carries, stated rather than
    /// implied: vanilla's `ClockInstance` defaults are `totalTicks = 0`,
    /// `partialTick = 0`, and **`rate = 1.0F`** — a zero rate would freeze it.
    #[test]
    fn a_minted_instance_has_rate_one() {
        let mut m = ClockManager::default();
        let _ = m.total_ticks(4);
        assert_eq!(
            *m.peek(4).unwrap(),
            WorldClock {
                total: 0,
                partial: 0.0,
                rate: 1.0,
                last_game_time: 0,
            }
        );
    }
}

mod clock_tests {
    use super::{ClockManager, WorldClock};

    const OVERWORLD: Option<i32> = Some(0);

    /// Seed a production [`ClockManager`] with the single clock these tests
    /// were written against.
    fn manager(clock: Option<WorldClock>, id: Option<i32>) -> ClockManager {
        let mut m = ClockManager::default();
        if let (Some(c), Some(id)) = (clock, id) {
            m.last_game_time = c.last_game_time;
            m.clocks.push((id, c));
        }
        m
    }

    /// The shape M12's tests were written against, now a **harness over the
    /// production `ClockManager`** rather than a function of its own.
    ///
    /// Every claim below still lands on `ClockManager::handle_updates` — the
    /// tick-then-overwrite order and all of `WorldClock::advance`'s
    /// arithmetic. What this wrapper does itself is the map bookkeeping, which
    /// is why that is graded separately by `clock_map_tests` rather than here:
    /// a harness cannot witness itself.
    fn apply_set_time(
        clock: &mut Option<WorldClock>,
        overworld_id: Option<i32>,
        game_time: i64,
        entries: &[(i32, i64, f32, f32)],
    ) {
        let mut m = manager(*clock, overworld_id);
        m.handle_updates(game_time, entries);
        *clock = overworld_id.and_then(|id| m.peek(id)).copied();
    }

    /// The same shape for the local `+1`, over the production function.
    fn local_tick_time(
        game_time: Option<i64>,
        clock: &mut Option<WorldClock>,
    ) -> Option<(i64, i64)> {
        let mut m = manager(*clock, OVERWORLD);
        let out = super::local_tick_time(game_time, &mut m, OVERWORLD);
        *clock = OVERWORLD.and_then(|id| m.peek(id)).copied();
        out
    }

    /// The join packet establishes the clock from an explicit state — total and
    /// last-game-time come straight from the wire. (The real server session
    /// showed `game=138341` establishing `total=189121`.)
    #[test]
    fn initial_explicit_state_establishes_the_clock() {
        let mut clock = None;
        apply_set_time(&mut clock, OVERWORLD, 138341, &[(0, 189121, 0.0, 1.0)]);
        let c = clock.expect("clock established");
        assert_eq!(c.total, 189121);
        assert_eq!(c.last_game_time, 138341);
        assert_eq!(c.partial, 0.0);
        assert_eq!(c.rate, 1.0);
    }

    /// The 20-tick `forceGameTimeSynchronization` sync carries an EMPTY map; at
    /// rate 1 it must advance `total` by the exact game-time delta. This is the
    /// frozen-clock regression: the old code held the last total here.
    #[test]
    fn empty_map_advances_by_the_game_time_delta_at_rate_one() {
        let mut clock = Some(WorldClock::from_state(138341, 189121, 0.0, 1.0));
        // The real diagnostic's game times after the join, deltas 3/20/20/20.
        for (game_time, expected_total) in [
            (138344, 189124),
            (138364, 189144),
            (138384, 189164),
            (138404, 189184),
        ] {
            apply_set_time(&mut clock, OVERWORLD, game_time, &[]);
            let c = clock.unwrap();
            assert_eq!(c.total, expected_total, "at game {game_time}");
            assert_eq!(c.last_game_time, game_time);
        }
    }

    /// A paused world (`/tick freeze`, or `doDaylightCycle false` reported as
    /// rate 0) must NOT advance on empty syncs, however large the delta.
    #[test]
    fn paused_rate_zero_holds_total() {
        let mut clock = Some(WorldClock::from_state(1000, 500, 0.0, 0.0));
        apply_set_time(&mut clock, OVERWORLD, 1020, &[]);
        apply_set_time(&mut clock, OVERWORLD, 5000, &[]);
        let c = clock.unwrap();
        assert_eq!(c.total, 500, "paused clock frozen");
        assert_eq!(c.last_game_time, 5000, "still anchors to gameTime");
    }

    /// A fractional rate proves the floor + partial carry: at rate 0.5 a
    /// single-tick advance banks half a tick, and the second single tick rolls
    /// the carry over into one whole `total` tick.
    #[test]
    fn fractional_rate_floors_and_carries_the_remainder() {
        let mut clock = Some(WorldClock::from_state(0, 0, 0.0, 0.5));

        apply_set_time(&mut clock, OVERWORLD, 1, &[]); // +0.5 → floor 0, carry 0.5
        let c = clock.unwrap();
        assert_eq!(c.total, 0, "half a tick banks nothing yet");
        assert!((c.partial - 0.5).abs() < 1e-9, "carry {}", c.partial);

        apply_set_time(&mut clock, OVERWORLD, 2, &[]); // 0.5 + 0.5 = 1.0 → floor 1
        let c = clock.unwrap();
        assert_eq!(c.total, 1, "carry rolls into one whole tick");
        assert!(c.partial.abs() < 1e-9, "carry reset, got {}", c.partial);
    }

    /// A negative `rate` (a clock running backward) proves the floor rounds
    /// toward negative infinity, not toward zero: starting at partial 0.25, one
    /// tick at rate -0.5 gives newPartial -0.25, `floor(-0.25) == -1` (NOT 0), so
    /// `total` BORROWS a whole tick (10 → 9) and the remainder is a POSITIVE
    /// `-0.25 - (-1) == 0.75`. A truncate-toward-zero floor would wrongly leave
    /// `total` at 10 with a negative carry.
    #[test]
    fn negative_rate_floors_toward_negative_infinity_with_positive_carry() {
        let mut clock = Some(WorldClock::from_state(0, 10, 0.25, -0.5));

        apply_set_time(&mut clock, OVERWORLD, 1, &[]); // 0.25 - 0.5 = -0.25 → floor -1
        let c = clock.unwrap();
        assert_eq!(c.total, 9, "borrowed one whole tick from total");
        assert!(
            (c.partial - 0.75).abs() < 1e-6,
            "positive carry {}",
            c.partial
        );
        assert_eq!(c.last_game_time, 1);
    }

    /// Vanilla `handleUpdates` order: a packet that both advances (non-empty
    /// game-time delta) AND carries an explicit overworld state must land on the
    /// explicit value — the advance happens first and is then overwritten.
    #[test]
    fn explicit_state_overwrites_after_the_advance() {
        let mut clock = Some(WorldClock::from_state(100, 5000, 0.0, 1.0));
        // Delta 20 would advance to 5020, but the explicit reset wins.
        apply_set_time(&mut clock, OVERWORLD, 120, &[(0, 0, 0.0, 1.0)]);
        let c = clock.unwrap();
        assert_eq!(c.total, 0, "explicit /time set overrides the advance");
        assert_eq!(c.last_game_time, 120);
    }

    /// The_end's clock is present in the map but must not touch the overworld —
    /// entries are matched by registry id, and the overworld still advances.
    #[test]
    fn a_non_overworld_entry_only_advances_the_overworld() {
        let mut clock = Some(WorldClock::from_state(100, 5000, 0.0, 1.0));
        // id 1 is the_end; the overworld advances by the delta, id 1 ignored.
        apply_set_time(&mut clock, OVERWORLD, 120, &[(1, 999, 0.0, 1.0)]);
        assert_eq!(
            clock.unwrap().total,
            5020,
            "overworld advanced, the_end skipped"
        );
    }

    /// `Mth.floor` returns a Java `int`, so a `newPartialTicks` past `i32::MAX`
    /// must saturate `fullTicks` to `i32::MAX` — NOT the far larger `i64::MAX` a
    /// direct `f64 as i64` cast would give. At rate 1 a 5-billion-tick delta
    /// (well past `i32::MAX ≈ 2.147e9`) banks exactly `i32::MAX` whole ticks and
    /// carries the ~2.85e9 remainder against that saturated value. The buggy
    /// direct-i64 path would instead bank the full 5e9 and carry 0.
    #[test]
    fn huge_positive_partial_saturates_full_to_i32_max() {
        let mut clock = Some(WorldClock::from_state(0, 0, 0.0, 1.0));
        // delta = 5_000_000_000 (exact in f64), newPartialTicks = 5e9.
        apply_set_time(&mut clock, OVERWORLD, 5_000_000_000, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total,
            i64::from(i32::MAX),
            "double→int saturates to i32::MAX, not i64::MAX (direct i64 would be 5e9)"
        );
        assert!(
            c.partial.is_finite() && c.partial > 2.8e9,
            "remainder taken against the i32-saturated full, not the true floor (got {})",
            c.partial
        );
        assert_eq!(c.last_game_time, 5_000_000_000);
    }

    /// A `NaN` rate poisons the arithmetic: `newPartialTicks` is `NaN`,
    /// `Mth.floor`'s `double→int` narrowing maps `NaN` to `0` (so `total` holds),
    /// and the `(float)(NaN - 0)` carry is `NaN`. This must not panic.
    #[test]
    fn nan_rate_floors_to_zero_and_poisons_the_carry() {
        let mut clock = Some(WorldClock::from_state(0, 100, 0.0, f32::NAN));
        apply_set_time(&mut clock, OVERWORLD, 1, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total, 100,
            "NaN floors to 0 (NaN→int is 0), total unchanged"
        );
        assert!(
            c.partial.is_nan(),
            "NaN rate poisons the carry, got {}",
            c.partial
        );
        assert_eq!(c.last_game_time, 1);
    }

    /// `gameTime - lastTickGameTime` is `long` subtraction that wraps
    /// two's-complement — `i64::MAX - i64::MIN` wraps to `-1`, not a debug panic.
    /// At rate 1 that -1 delta borrows exactly one whole tick from `total`.
    #[test]
    fn delta_subtraction_wraps_rather_than_panics() {
        let mut clock = Some(WorldClock::from_state(i64::MIN, 5000, 0.0, 1.0));
        // i64::MAX.wrapping_sub(i64::MIN) == -1 → newPartialTicks = -1.0.
        apply_set_time(&mut clock, OVERWORLD, i64::MAX, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total, 4999,
            "wrapped delta of -1 borrows one tick, no panic"
        );
        assert_eq!(c.last_game_time, i64::MAX);
    }

    /// `totalTicks += fullTicks` is `long` addition that wraps two's-complement —
    /// `i64::MAX + 1` wraps to `i64::MIN`, not a debug overflow panic.
    #[test]
    fn total_addition_wraps_rather_than_panics() {
        let mut clock = Some(WorldClock::from_state(0, i64::MAX, 0.0, 1.0));
        // delta 1 at rate 1 → fullTicks 1 → i64::MAX.wrapping_add(1).
        apply_set_time(&mut clock, OVERWORLD, 1, &[]);
        let c = clock.unwrap();
        assert_eq!(
            c.total,
            i64::MIN,
            "wrapping_add overflow wraps to i64::MIN, no panic"
        );
        assert_eq!(c.last_game_time, 1);
    }

    // -- `ClientLevel.tickTime`: the local per-tick advance -----------------

    /// Before the first `set_time` the client game-time is `None`, so
    /// `ClientLevel.tickTime` has nothing to run — the local tick is a no-op and
    /// the renderer keeps reading full daylight.
    #[test]
    fn local_tick_before_first_set_time_is_a_no_op() {
        let mut clock = None;
        assert_eq!(local_tick_time(None, &mut clock), None);
        assert!(clock.is_none());
    }

    /// After an explicit join establishes the clock, each running client tick
    /// advances it by exactly one — N local ticks add N. This is the path the
    /// sync-only clock lacked: it moved in 20-tick jumps and fell short of the
    /// elapsed ticks (the measured +75 over 80 ticks).
    #[test]
    fn join_then_n_local_ticks_advance_n() {
        let mut clock = None;
        // Join carries an explicit overworld state; `set_time` also anchors the
        // client game-time to the packet value (the `setGameTime` step).
        apply_set_time(&mut clock, OVERWORLD, 1000, &[(0, 5000, 0.0, 1.0)]);
        let mut game_time = Some(1000);
        for i in 1..=64 {
            let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
            game_time = Some(gt);
            assert_eq!(gt, 1000 + i, "game_time advances one per tick");
            assert_eq!(day, 5000 + i, "day_ticks advances one per tick at rate 1");
        }
        assert_eq!(clock.unwrap().total, 5064);
        assert_eq!(game_time, Some(1064));
    }

    /// The no-double-count invariant: the 20-tick `forceGameTimeSynchronization`
    /// sync at the game time the client already predicted contributes a
    /// zero-delta advance on its own, leaving exactly the one local `+1` for
    /// that tick. (In `PlaySession::tick` the sync is drained before the local
    /// advance, so both run against the same clock in one tick.)
    #[test]
    fn empty_sync_at_predicted_time_adds_zero_then_one_local_tick() {
        // The client has locally ticked its clock up to game 1020 (clock 5020).
        let mut clock = Some(WorldClock::from_state(1020, 5020, 0.0, 1.0));
        let game_time = Some(1020);

        // A drained empty sync carries the *already-predicted* game time 1020 →
        // `setGameTime` is a no-op and `handleUpdates` advances by delta 0.
        apply_set_time(&mut clock, OVERWORLD, 1020, &[]);
        assert_eq!(
            clock.unwrap().total,
            5020,
            "empty sync at the predicted time adds zero"
        );

        // Then this tick's single local `+1`.
        let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
        assert_eq!(gt, 1021);
        assert_eq!(day, 5021, "exactly one tick banked, no double count");
    }

    /// A server sync whose game time DIFFERS from the client's prediction
    /// re-anchors the clock to the server value (a forward jump banks the gap, a
    /// backward jump borrows it), and the same tick's local `+1` then continues
    /// from the corrected value.
    #[test]
    fn server_correction_reanchors_then_local_tick() {
        // Forward correction: the client predicted 1000, the server is at 1005.
        let mut clock = Some(WorldClock::from_state(1000, 5000, 0.0, 1.0));
        apply_set_time(&mut clock, OVERWORLD, 1005, &[]); // advance delta +5
        assert_eq!(
            clock.unwrap().total,
            5005,
            "forward re-anchor banks the 5-tick gap"
        );
        let (gt, day) = local_tick_time(Some(1005), &mut clock).unwrap();
        assert_eq!(
            (gt, day),
            (1006, 5006),
            "local +1 continues from the correction"
        );

        // Backward correction: the client predicted 2000, the server is at 1997.
        let mut clock = Some(WorldClock::from_state(2000, 9000, 0.0, 1.0));
        apply_set_time(&mut clock, OVERWORLD, 1997, &[]); // advance delta -3
        assert_eq!(
            clock.unwrap().total,
            8997,
            "backward re-anchor borrows the 3-tick gap"
        );
        let (gt, day) = local_tick_time(Some(1997), &mut clock).unwrap();
        assert_eq!(
            (gt, day),
            (1998, 8998),
            "local +1 continues from the correction"
        );
    }

    /// A paused world (rate 0) advances the client game-time counter every tick
    /// but leaves the clock `total` frozen — the day/night cycle holds while the
    /// world keeps counting ticks (and the clock still re-anchors to `gameTime`).
    #[test]
    fn rate_zero_holds_total_while_game_time_advances() {
        let mut clock = Some(WorldClock::from_state(1000, 500, 0.0, 0.0));
        let mut game_time = Some(1000);
        for i in 1..=10 {
            let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
            game_time = Some(gt);
            assert_eq!(gt, 1000 + i, "game_time still counts up");
            assert_eq!(day, 500, "paused clock frozen");
        }
        let c = clock.unwrap();
        assert_eq!(c.total, 500);
        assert_eq!(c.last_game_time, 1010, "clock still re-anchors to gameTime");
    }

    /// The local `+1` is Java `long` addition (`getGameTime() + 1L`) and wraps
    /// two's-complement: at `i64::MAX` the next tick's game-time is `i64::MIN`,
    /// no debug-overflow panic. The clock's own wrapping delta then reads +1
    /// across the boundary (`i64::MIN.wrapping_sub(i64::MAX) == 1`).
    #[test]
    fn local_game_time_wraps_at_i64_max() {
        let mut clock = Some(WorldClock::from_state(i64::MAX, 5000, 0.0, 1.0));
        let (gt, day) = local_tick_time(Some(i64::MAX), &mut clock).unwrap();
        assert_eq!(gt, i64::MIN, "game_time wraps to i64::MIN, no panic");
        assert_eq!(
            day, 5001,
            "clock's wrapping delta reads one tick across the wrap"
        );
        assert_eq!(clock.unwrap().last_game_time, i64::MIN);
    }

    /// Best-effort fallback: a server that sends `set_time` but never an
    /// overworld clock state leaves `overworld_clock` `None`; the local tick
    /// still advances the day-tick by falling back to the raw game time (one per
    /// tick), rather than only jumping on packets.
    #[test]
    fn no_explicit_clock_falls_back_to_game_time_each_tick() {
        let mut clock: Option<WorldClock> = None;
        // `set_time` with an entry for the_end (id 1) only — overworld unmatched.
        apply_set_time(&mut clock, OVERWORLD, 200, &[(1, 999, 0.0, 1.0)]);
        assert!(clock.is_none(), "no overworld clock established");
        let mut game_time = Some(200);
        for i in 1..=5 {
            let (gt, day) = local_tick_time(game_time, &mut clock).unwrap();
            game_time = Some(gt);
            assert_eq!(day, 200 + i, "fallback day-tick advances one per tick");
        }
    }
}

mod ping_tests {
    //! M52c — the ping the client can actually know.
    //!
    //! These build the `player_info_update` body by hand and run it through
    //! the production `apply_player_info`, so the action bitmask, the entry
    //! walk and the latency slot are all exercised together. A local
    //! reimplementation would pass while the real decoder desynced.

    use super::*;

    /// Encode a var-int the way the wire does.
    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    /// A body carrying UPDATE_LATENCY (action bit 4) for one uuid.
    fn latency_body(entries: &[(u128, i32)]) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(1u8 << 4); // only UPDATE_LATENCY
        varint(&mut b, entries.len() as i32);
        for (uuid, ms) in entries {
            b.extend_from_slice(&uuid.to_be_bytes());
            varint(&mut b, *ms);
        }
        b
    }

    #[test]
    fn update_latency_is_parsed_rather_than_discarded() {
        assert_eq!(parse_player_info_latency(&latency_body(&[(7, 42)])), [(7, 42)]);
    }

    #[test]
    fn several_players_are_walked_independently() {
        // The walk advances uuid-then-latency per entry; a mis-sized skip
        // corrupts every entry after it rather than failing.
        assert_eq!(
            parse_player_info_latency(&latency_body(&[(1, 10), (2, 250), (3, 0)])),
            [(1, 10), (2, 250), (3, 0)],
            "a reported zero is a value, not unknown"
        );
    }

    #[test]
    fn a_negative_latency_is_a_state_not_a_decode_error() {
        // PlayerTabOverlay buckets latency < 0 into the no-connection icon, so
        // the wire really does carry negatives; clamping at decode would erase
        // a state vanilla renders.
        assert_eq!(parse_player_info_latency(&latency_body(&[(9, -1)])), [(9, -1)]);
    }

    #[test]
    fn an_unset_latency_action_yields_nothing() {
        // Sensitivity partner: a mask without bit 4 must not invent an entry.
        // Reading the field unconditionally would fabricate a ping AND desync
        // the walk.
        let mut b = Vec::new();
        b.push(1u8 << 3);
        varint(&mut b, 1);
        b.extend_from_slice(&7u128.to_be_bytes());
        b.push(1);
        assert!(parse_player_info_latency(&b).is_empty());
    }

    #[test]
    fn an_action_before_latency_must_be_walked_first() {
        // LISTED (3) then LATENCY (4). Skipping the bool makes the walk read
        // it AS the varint and report 1ms -- a plausible number, which is
        // what makes it dangerous.
        let mut b = Vec::new();
        b.push((1u8 << 3) | (1u8 << 4));
        varint(&mut b, 1);
        b.extend_from_slice(&7u128.to_be_bytes());
        b.push(1);
        varint(&mut b, 200);
        assert_eq!(parse_player_info_latency(&b), [(7, 200)]);
    }
}

mod player_info_field_tests {
    //! M62 — the two `player_info_update` fields the tab list's first two
    //! sort keys come from: `UPDATE_GAME_MODE` (action 2) and
    //! `UPDATE_LIST_ORDER` (action 6). Both were read into a discard.
    //!
    //! Every body is built by hand and run through the production
    //! `parse_player_info`, so the bitmask and the entry walk are what is
    //! under test.

    use super::*;

    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    /// A one-entry body carrying exactly the actions in `mask`, with each
    /// set action's payload appended by `fields` in bit order.
    fn one_entry(mask: u8, uuid: u128, fields: &[u8]) -> Vec<u8> {
        let mut b = vec![mask];
        varint(&mut b, 1);
        b.extend_from_slice(&uuid.to_be_bytes());
        b.extend_from_slice(fields);
        b
    }

    #[test]
    fn the_game_mode_action_is_kept_rather_than_discarded() {
        let (e, res) = parse_player_info(&one_entry(1 << 2, 7, &[3]));
        assert!(res.is_ok());
        assert_eq!(e[0].gamemode, Some(GameMode::Spectator));
        assert!(e[0].gamemode.unwrap().is_spectator());
    }

    #[test]
    fn the_tab_list_order_action_is_kept_rather_than_discarded() {
        let mut f = Vec::new();
        varint(&mut f, 42);
        let (e, res) = parse_player_info(&one_entry(1 << 6, 7, &f));
        assert!(res.is_ok());
        assert_eq!(e[0].tab_list_order, Some(42));
    }

    #[test]
    fn an_out_of_range_game_mode_id_is_survival_rather_than_an_error() {
        // `GameType.byId` is ByIdMap.continuous(..., ZERO), so 9 -> values[0].
        // An error here would drop a packet vanilla renders fine.
        let (e, res) = parse_player_info(&one_entry(1 << 2, 7, &[9]));
        assert!(res.is_ok());
        assert_eq!(e[0].gamemode, Some(GameMode::Survival));
    }

    #[test]
    fn an_unset_action_leaves_the_field_absent_rather_than_defaulted() {
        // The sensitivity partner for both. The packet is a DELTA: filling in
        // `Survival` / `0` here would tell the tab list a spectator had
        // switched to survival on every latency-only update, and the sort
        // would visibly reshuffle.
        let mut f = Vec::new();
        varint(&mut f, 55);
        let (e, _) = parse_player_info(&one_entry(1 << 4, 7, &f));
        assert_eq!(e[0].latency, Some(55));
        assert_eq!(e[0].gamemode, None);
        assert_eq!(e[0].tab_list_order, None);
    }

    #[test]
    fn a_mis_sized_earlier_action_would_report_a_plausible_wrong_order() {
        // GAME_MODE (2) then LIST_ORDER (6), with a two-byte var-int mode so
        // a one-byte skip is observable. Read correctly the order is 7; a
        // walk that assumed a single byte reads the mode's continuation byte
        // as the order and reports 1 -- a number nothing downstream can
        // reject.
        let mut f = Vec::new();
        varint(&mut f, 129); // two bytes: 0x81 0x01 -> mode id 129, ZERO -> Survival
        varint(&mut f, 7);
        let body = one_entry((1 << 2) | (1 << 6), 7, &f);
        let (e, res) = parse_player_info(&body);
        assert!(res.is_ok());
        assert_eq!(e[0].gamemode, Some(GameMode::Survival));
        assert_eq!(e[0].tab_list_order, Some(7));

        // The mis-sized walk, run over the same bytes.
        let mut r = PacketReader::new(&body);
        let _ = r.u8().unwrap();
        let _ = r.count("player info entries", 16).unwrap();
        let _ = r.uuid().unwrap();
        let _ = r.u8().unwrap(); // one byte where the mode is two
        assert_eq!(
            r.varint().unwrap(),
            1,
            "the mis-sized walk must report a plausible wrong order, not fail"
        );
    }

    #[test]
    fn several_entries_carry_their_own_values() {
        // Two entries under one mask, which is the shape a real join sends.
        // A walk that lost a byte in the first entry would attribute the
        // second's fields to the wrong uuid.
        let mut b = vec![(1u8 << 2) | (1u8 << 6)];
        varint(&mut b, 2);
        b.extend_from_slice(&1u128.to_be_bytes());
        b.push(3); // spectator
        varint(&mut b, 10);
        b.extend_from_slice(&2u128.to_be_bytes());
        b.push(1); // creative
        varint(&mut b, 20);

        let (e, res) = parse_player_info(&b);
        assert!(res.is_ok());
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].uuid, e[0].gamemode, e[0].tab_list_order), (1, Some(GameMode::Spectator), Some(10)));
        assert_eq!((e[1].uuid, e[1].gamemode, e[1].tab_list_order), (2, Some(GameMode::Creative), Some(20)));
    }

    #[test]
    fn a_truncated_entry_keeps_the_fields_it_completed() {
        // The body promises a mode and an order and stops after the mode.
        // The completed field must survive, because that is what the
        // pre-M62 field-at-a-time decoder did and losing it would silently
        // discard a whole packet's worth of state on one short read.
        let mut b = vec![(1u8 << 2) | (1u8 << 6)];
        varint(&mut b, 1);
        b.extend_from_slice(&7u128.to_be_bytes());
        b.push(3);
        let (e, res) = parse_player_info(&b);
        assert!(res.is_err());
        assert_eq!(e[0].gamemode, Some(GameMode::Spectator));
        assert_eq!(e[0].tab_list_order, None);
    }
}

mod m151_tab_list_fields {
    //! M151 — the three `player_info_update` actions the tab list needs and
    //! M62 left decoded-and-dropped: `UPDATE_LISTED` (3), `UPDATE_DISPLAY_NAME`
    //! (5) and `UPDATE_HAT` (7).
    //!
    //! Every body is built by hand and run through the production
    //! `parse_player_info`, so the bitmask and the entry walk are the subject —
    //! and `UPDATE_DISPLAY_NAME` is the one action in this packet whose payload
    //! is a variable-length component, so a walk that read it wrongly would
    //! desynchronise everything after it rather than merely mis-report a field.

    use super::*;
    use rewo_proto::nbt::Nbt;

    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    fn one_entry(mask: u8, uuid: u128, fields: &[u8]) -> Vec<u8> {
        let mut b = vec![mask];
        varint(&mut b, 1);
        b.extend_from_slice(&uuid.to_be_bytes());
        b.extend_from_slice(fields);
        b
    }

    /// A network-NBT bare `TAG_String` — the shape a trusted component takes
    /// on the wire.
    fn nbt_string_bytes(s: &str) -> Vec<u8> {
        let mut b = vec![8u8];
        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
        b
    }

    #[test]
    fn the_listed_action_is_kept_rather_than_discarded() {
        let (e, res) = parse_player_info(&one_entry(1 << 3, 7, &[1]));
        assert!(res.is_ok());
        assert_eq!(e[0].listed, Some(true));
        let (e, res) = parse_player_info(&one_entry(1 << 3, 7, &[0]));
        assert!(res.is_ok());
        assert_eq!(e[0].listed, Some(false));
    }

    #[test]
    fn the_hat_action_is_kept_rather_than_discarded() {
        let (e, res) = parse_player_info(&one_entry(1 << 7, 7, &[0]));
        assert!(res.is_ok());
        assert_eq!(e[0].show_hat, Some(false));
    }

    /// The double `Option`, both halves.
    ///
    /// The field is `optional(TRUSTED_STREAM_CODEC)`, so a present action can
    /// still carry a null — and `applyPlayerInfoUpdate` assigns it
    /// unconditionally, which is how a server takes a custom name back down.
    #[test]
    fn a_present_display_name_action_can_still_carry_a_null() {
        let mut f = vec![1u8];
        f.extend_from_slice(&nbt_string_bytes("Boss"));
        let (e, res) = parse_player_info(&one_entry(1 << 5, 7, &f));
        assert!(res.is_ok());
        assert_eq!(e[0].display_name, Some(Some(Nbt::String("Boss".into()))));

        // Present action, null field: "clear it".
        let (e, res) = parse_player_info(&one_entry(1 << 5, 7, &[0]));
        assert!(res.is_ok());
        assert_eq!(e[0].display_name, Some(None));

        // Absent action: "unchanged". Three states, and collapsing any two of
        // them loses a behaviour a real server uses.
        let (e, _) = parse_player_info(&one_entry(1 << 3, 7, &[1]));
        assert_eq!(e[0].display_name, None);
    }

    /// The desynchronising case: a display name is variable-length, so an
    /// action after it reads garbage if the component is not fully walked.
    ///
    /// LIST_ORDER (6) follows DISPLAY_NAME (5). A walk that skipped the
    /// component's body would read the NBT tag byte 8 as the order — a
    /// plausible number nothing downstream can reject.
    #[test]
    fn an_action_after_the_display_name_needs_the_component_fully_walked() {
        let mut f = vec![1u8];
        f.extend_from_slice(&nbt_string_bytes("Boss"));
        varint(&mut f, 77);
        let (e, res) = parse_player_info(&one_entry((1 << 5) | (1 << 6), 7, &f));
        assert!(res.is_ok());
        assert_eq!(e[0].tab_list_order, Some(77));

        // The skipping walk, over the same bytes: it lands on the tag byte.
        let body = one_entry((1 << 5) | (1 << 6), 7, &f);
        let mut r = PacketReader::new(&body);
        let _ = r.u8().unwrap();
        let _ = r.count("player info entries", 16).unwrap();
        let _ = r.uuid().unwrap();
        let _ = r.bool().unwrap();
        assert_eq!(
            r.varint().unwrap(),
            8,
            "a walk that stopped at the presence byte reports the NBT tag as the order"
        );
    }

    /// The production state application, driven by production-decoded bodies.
    ///
    /// **`TabListPlayers::apply`, not a copy of it.** The first cut of this
    /// test reimplemented the set arithmetic inline, and M151's mutation
    /// battery duly reported "UPDATE_LISTED only ever ADDS" as SURVIVED — the
    /// test was grading its own copy while the client ran the other one, which
    /// is M45's `install_shapes` shape. The arithmetic moved out of
    /// `PlaySession` (unreachable from any test — M71) so this could call it.
    fn apply_body(t: &mut TabListPlayers, body: &[u8]) {
        for e in parse_player_info(body).0 {
            t.apply(&e);
        }
    }

    /// `UPDATE_LISTED`'s arm is an add/remove on a set, so `false` for someone
    /// never added is a no-op and there is no stored third state.
    #[test]
    fn listed_is_a_set_membership_and_defaults_to_absent() {
        let mut t = TabListPlayers::default();
        // A player described by a latency-only update is NOT listed: the
        // default is exclusion, which is what keeps a vanished player off the
        // list while their skin and team still resolve.
        let mut f = Vec::new();
        varint(&mut f, 40);
        apply_body(&mut t, &one_entry(1 << 4, 7, &f));
        assert!(!t.listed.contains(&7));

        apply_body(&mut t, &one_entry(1 << 3, 7, &[1]));
        assert!(t.listed.contains(&7));
        assert_eq!(t.listed_players(), [7]);
        // …and a later `false` REMOVES. A client that only ever added would
        // keep a vanished player on the list forever.
        apply_body(&mut t, &one_entry(1 << 3, 7, &[0]));
        assert!(!t.listed.contains(&7));
        assert!(t.listed_players().is_empty());
        // A second `false` is inert rather than an error.
        apply_body(&mut t, &one_entry(1 << 3, 7, &[0]));
        assert!(!t.listed.contains(&7));
    }

    /// `handlePlayerInfoRemove` takes the uuid out of `listedPlayers` as well
    /// as out of `playerInfoMap`. Nothing else ever removes one.
    #[test]
    fn forgetting_a_player_takes_them_off_the_list() {
        let mut t = TabListPlayers::default();
        let mut f = vec![1u8]; // listed
        f.push(1); // showHat
        apply_body(&mut t, &one_entry((1 << 3) | (1 << 7), 7, &f));
        assert_eq!(t.listed_players(), [7]);
        t.forget(7);
        assert!(t.listed_players().is_empty());
        assert!(t.show_hats.is_empty());
    }

    /// `PlayerInfo.showHat` initialises to **true** (`PlayerInfo.java:21`).
    ///
    /// The one absent value in this type that does not mean "the server has
    /// not said": there is no question a caller could ask that unsent answers
    /// differently from shown, which is why the accessor owns the default
    /// rather than the map.
    #[test]
    fn an_unsent_hat_reads_as_shown() {
        let mut t = TabListPlayers::default();
        assert!(t.show_hat(7), "a player nobody described wears their hat");
        apply_body(&mut t, &one_entry(1 << 7, 7, &[0]));
        assert!(!t.show_hat(7));
        apply_body(&mut t, &one_entry(1 << 7, 7, &[1]));
        assert!(t.show_hat(7));
        // Still true for anyone else, which is what says the map is per-uuid
        // and not a global flag.
        assert!(t.show_hat(9));
    }

    /// A display override arrives, then a null one clears it.
    #[test]
    fn a_null_display_name_clears_through_the_production_apply() {
        let mut t = TabListPlayers::default();
        let mut f = vec![1u8];
        f.push(8); // TAG_String
        f.extend_from_slice(&4u16.to_be_bytes());
        f.extend_from_slice(b"Boss");
        apply_body(&mut t, &one_entry(1 << 5, 7, &f));
        assert!(t.display_name(7).is_some());
        apply_body(&mut t, &one_entry(1 << 5, 7, &[0]));
        assert!(t.display_name(7).is_none());
    }
}

mod m146_music {
    use super::situational_music_from;
    use rewo_world::music::{musics, BackgroundMusic};

    /// **The End boss beats the biome, and only in the End.**
    #[test]
    fn the_dragon_needs_both_the_dimension_and_the_bar() {
        let bg = BackgroundMusic::of_sound("minecraft:music.game.end");
        // Both: the dragon.
        assert_eq!(
            situational_music_from(true, true, &bg, false, false).unwrap().sound,
            "minecraft:music.dragon"
        );
        // A boss bar elsewhere — a wither in the Overworld — is the biome's.
        assert_eq!(
            situational_music_from(false, true, &bg, false, false).unwrap().sound,
            "minecraft:music.game.end"
        );
        // The End with no boss is also the biome's.
        assert_eq!(
            situational_music_from(true, false, &bg, false, false).unwrap().sound,
            "minecraft:music.game.end"
        );
    }

    /// The selection flags reach `select`, in its own priority order.
    #[test]
    fn creative_and_underwater_reach_the_selection() {
        let bg = BackgroundMusic::overworld().with_underwater(musics::under_water());
        let pick = |c, u| {
            situational_music_from(false, false, &bg, c, u)
                .unwrap()
                .sound
        };
        assert_eq!(pick(false, false), "minecraft:music.game");
        assert_eq!(pick(true, false), "minecraft:music.creative");
        assert_eq!(pick(false, true), "minecraft:music.under_water");
        assert_eq!(pick(true, true), "minecraft:music.under_water", "water wins");
    }

    /// A place that offers nothing offers nothing — **not** the game track.
    ///
    /// `orElse(null)` is the last word in vanilla's method, and an empty record
    /// really is reachable: `OverworldBiomes.java:596` declares one.
    #[test]
    fn an_empty_record_is_silence_rather_than_a_default() {
        assert!(situational_music_from(false, false, &BackgroundMusic::empty(), false, false).is_none());
        // …and the dragon still overrides it, because that arm returns before
        // the record is consulted at all.
        assert!(situational_music_from(true, true, &BackgroundMusic::empty(), false, false).is_some());
    }
}

/// The elytra take-off line (`LocalPlayer.aiStep:850`). A `PlaySession` owns a
/// socket and cannot be built in a test (M71's lesson), so these witness the
/// pure function `PlaySession::tick` adapts — with a real inventory, and with
/// the real glider classification behind it.
mod fall_flying_tests {
    use super::*;
    use rewo_world::inventory::{Inventory, ItemSlot};

    /// The id table `into_play` resolves glider ids against. `Items::for_tests`
    /// numbers by position, so `minecraft:elytra` is id 0 in this fixture.
    fn items() -> rewo_data::items::Items {
        rewo_data::items::Items::for_tests(&["minecraft:elytra", "minecraft:bread"])
    }

    /// One take-off attempt, returning the bytes it would send. The player is
    /// `PlayerState::at`'s airborne, dry default, so
    /// [`physics::can_start_fall_flying`]'s own clauses hold except the two
    /// this passes in — those are covered exhaustively by the physics side's
    /// `can_start_fall_flying_is_the_java_truth_table`.
    fn attempt(
        jump: bool,
        was_jumping: bool,
        just_toggled_creative_flight: bool,
        on_climbable: bool,
        fall_flying: bool,
        elytra_in_chest: bool,
    ) -> Option<Vec<u8>> {
        let items = items();
        let glider_items = glider_item_ids(&items);
        let mut inventory = Inventory::default();
        if elytra_in_chest {
            let elytra = items.id("minecraft:elytra").expect("the fixture names it");
            // Inventory index 38 is the chest armour slot — menu slot 6, the
            // `ARMOR_MENU_START + 1` lookup `chest_is_glider` makes.
            let _ = inventory.set_inventory_index(38, Some(ItemSlot::plain(elytra, 1)));
        }
        let state = PlayerState::at(0.0, 64.0, 0.0);
        let abilities = rewo_world::abilities::Abilities::default();
        let attrs = physics::MoveAttributes::default();
        fall_flying_takeoff(&FallFlyingTakeoff {
            jump,
            was_jumping,
            just_toggled_creative_flight,
            on_climbable,
            state: &state,
            abilities: &abilities,
            attrs: &attrs,
            fall_flying,
            inventory: &inventory,
            glider_items: &glider_items,
            wire: Some((0x25, 7)),
        })
        .map(|p| p.into_bytes())
    }

    /// Jump in the air with an elytra in the chest slot emits
    /// `ServerboundPlayerCommandPacket(7, START_FALL_FLYING)`; without one it
    /// emits nothing at all. The bytes are the packet id `0x25` as a varint,
    /// then `player_command_body`'s entity 7, action 6, data 0 — the last two
    /// being the `Action` ordinal and the two-argument constructor's `data`.
    #[test]
    fn jump_in_the_air_takes_off_only_with_an_elytra_in_the_chest_slot() {
        assert_eq!(
            attempt(true, false, false, false, false, true),
            Some(vec![0x25, 7, 6, 0]),
            "START_FALL_FLYING is the Action enum's seventh constant"
        );
        assert_eq!(attempt(true, false, false, false, false, false), None);
    }

    /// The line is a conjunction and every other conjunct is load-bearing: the
    /// key must be *newly* pressed (a held key does not take off), the press
    /// that just toggled creative flight is already spent, a climbable is the
    /// ladder's business, and an already-flying elytra is not started twice.
    #[test]
    fn the_take_off_line_is_a_conjunction() {
        assert_eq!(attempt(false, false, false, false, false, true), None, "no press");
        assert_eq!(attempt(true, true, false, false, false, true), None, "held, not newly pressed");
        assert_eq!(
            attempt(true, false, true, false, false, true),
            None,
            "the press that toggled creative flight"
        );
        assert_eq!(attempt(true, false, false, true, false, true), None, "on a climbable");
        assert_eq!(attempt(true, false, false, false, true, true), None, "already gliding");
    }

    /// The item test is the prototype's `minecraft:glider` component — which
    /// `minecraft:elytra` carries — applied to the **chest** slot, where
    /// `canGlideUsing`'s `equippable.slot() == slot` clause puts it. An elytra
    /// one slot up is the head and must not count.
    #[test]
    fn the_glider_test_is_the_prototype_component_in_the_chest_slot() {
        assert_eq!(
            rewo_data::item_components_table::prototype_has_component(
                "minecraft:elytra",
                "minecraft:glider"
            ),
            Some(true),
            "the classification `glider_item_ids` keys on"
        );
        let items = items();
        let glider_items = glider_item_ids(&items);
        let elytra = items.id("minecraft:elytra").unwrap();
        assert_eq!(glider_items, vec![elytra], "the bread carries no glider");
        let mut chest = Inventory::default();
        let _ = chest.set_inventory_index(38, Some(ItemSlot::plain(elytra, 1)));
        assert!(chest_is_glider(&chest, &glider_items));
        let mut head = Inventory::default();
        let _ = head.set_inventory_index(39, Some(ItemSlot::plain(elytra, 1)));
        assert!(!chest_is_glider(&head, &glider_items), "menu 5 is the head");
    }
}
