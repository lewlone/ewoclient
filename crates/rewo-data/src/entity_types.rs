//! registries.json → entity-type table. `add_entity` carries the type as a
//! protocol id into the `minecraft:entity_type` registry; rendering wants
//! the name back (is it a player? what capsule size / color?).

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

#[derive(Clone)]
pub struct EntityTypes {
    by_id: HashMap<i32, String>,
    pub player_id: i32,
}

impl EntityTypes {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::from_value(&read_json_file(path)?)
    }

    /// The same table from an already-parsed report, so a gate or a test can
    /// hand over a fixture without a file (M84).
    pub fn from_value(json: &serde_json::Value) -> Result<Self, String> {
        let entries = json
            .get("minecraft:entity_type")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:entity_type registry")?;
        let mut by_id = HashMap::with_capacity(entries.len());
        for (name, entry) in entries {
            if let Some(id) = entry.get("protocol_id").and_then(|i| i.as_i64()) {
                by_id.insert(id as i32, name.clone());
            }
        }
        let player_id = by_id
            .iter()
            .find(|(_, n)| n.as_str() == "minecraft:player")
            .map(|(id, _)| *id)
            .ok_or("registries.json: no minecraft:player entity type")?;
        log::info!(
            "rewo-data: {} entity types (player = {player_id})",
            by_id.len()
        );
        Ok(Self { by_id, player_id })
    }

    /// A table from literal pairs, for fixtures (M84). `minecraft:player` must
    /// be among them, because [`Self::player_id`] has no sensible default.
    pub fn from_pairs(pairs: &[(i32, &str)]) -> Result<Self, String> {
        let by_id: HashMap<i32, String> = pairs.iter().map(|(i, n)| (*i, n.to_string())).collect();
        let player_id = by_id
            .iter()
            .find(|(_, n)| n.as_str() == "minecraft:player")
            .map(|(id, _)| *id)
            .ok_or("EntityTypes::from_pairs: no minecraft:player")?;
        Ok(Self { by_id, player_id })
    }

    pub fn name(&self, id: i32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
    }

    /// Protocol id of a named entity type (`"minecraft:warden"`), or `None`
    /// if this version doesn't register it. Used to resolve the concrete
    /// kinds whose entity events this client interprets (warden, armadillo) —
    /// `ClientboundEntityEventPacket` bytes are polymorphic by entity class,
    /// so the id alone can't name the animation. Reverse of [`Self::name`].
    pub fn id_of(&self, name: &str) -> Option<i32> {
        self.by_id
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
    }

    /// Capsule footprint (width, height) in blocks for a type id. Exact for
    /// the common types; everything else gets the humanoid default — the v1
    /// capsule renderer doesn't need more (REWO_PLAN correction #11).
    pub fn dimensions(&self, id: i32) -> (f32, f32) {
        match self.name(id).unwrap_or("") {
            "minecraft:player" => (0.6, 1.8),
            "minecraft:zombie" | "minecraft:husk" | "minecraft:drowned" => (0.6, 1.95),
            "minecraft:skeleton" | "minecraft:stray" => (0.6, 1.99),
            "minecraft:creeper" => (0.6, 1.7),
            "minecraft:villager" => (0.6, 1.95),
            "minecraft:enderman" => (0.6, 2.9),
            "minecraft:cow" | "minecraft:mooshroom" => (0.9, 1.4),
            "minecraft:pig" => (0.9, 0.9),
            "minecraft:sheep" => (0.9, 1.3),
            "minecraft:chicken" => (0.4, 0.7),
            "minecraft:wolf" => (0.6, 0.85),
            "minecraft:armor_stand" => (0.5, 1.975),
            "minecraft:item" => (0.25, 0.25),
            _ => (0.6, 1.8),
        }
    }

    /// Whether this entity type shoves other entities out of the way.
    ///
    /// Vanilla `Entity.isPushable()` is **false** by default and only
    /// `LivingEntity` overrides it to true, so items, arrows, displays and
    /// armor stands never push. This is the same rule expressed as an
    /// exclusion list over the registry names, since the wire gives us type
    /// ids rather than class hierarchies.
    pub fn pushable(&self, id: i32) -> bool {
        let name = self.name(id).unwrap_or("");
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        // Projectiles, drops, markers and decorations — never pushable.
        const NOT_LIVING: &[&str] = &[
            "item",
            "experience_orb",
            "arrow",
            "spectral_arrow",
            "trident",
            "snowball",
            "egg",
            "ender_pearl",
            "eye_of_ender",
            "potion",
            "experience_bottle",
            "fireball",
            "small_fireball",
            "dragon_fireball",
            "wither_skull",
            "wind_charge",
            "breeze_wind_charge",
            "llama_spit",
            "shulker_bullet",
            "fishing_bobber",
            "firework_rocket",
            "tnt",
            "falling_block",
            "lightning_bolt",
            "area_effect_cloud",
            "painting",
            "item_frame",
            "glow_item_frame",
            "leash_knot",
            "marker",
            "interaction",
            "text_display",
            "block_display",
            "item_display",
            "armor_stand",
            "end_crystal",
            "evoker_fangs",
            "ominous_item_spawner",
        ];
        !NOT_LIVING.contains(&short)
    }

    /// Every registered protocol id (for the registry-size drift check).
    pub fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.by_id.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// Which entity types are `LivingEntity`s, and which of them run
/// `LivingEntity.updateSwingTime()` on the client — resolved from the
/// machine-extracted [`crate::entity_classes`] name tables against the live
/// registry (M19).
///
/// Both questions are Java-class facts that no datagen report carries, and both
/// are load-bearing:
///
/// * **Living** gates every swing input. `handleAnimate` casts to
///   `LivingEntity`, `handleSetEquipment` and `handleUpdateMobEffect` test
///   `instanceof LivingEntity` — a packet naming a boat or an arrow must mutate
///   nothing at all.
/// * **Swing-ticking** decides whose `attackAnim` advances.
///   `updateSwingTime` is not in `LivingEntity.tick`: on the client only
///   `Player.aiStep`, `RemotePlayer.tick`, `Monster.aiStep` and
///   `Mannequin.tick` call it, so a hoglin can be sent `swing()` (via
///   `Mob.doHurtTarget`) and never animate. This matters beyond the built-in
///   humanoid pose because OptiFine CEM publishes `swing_progress` for *every*
///   mob.
pub struct EntityClasses {
    living: std::collections::HashSet<i32>,
    swing_ticking: std::collections::HashSet<i32>,
    /// `Mob` descendants — `DATA_MOB_FLAGS_ID` at index 15 BYTE (M20).
    mob: std::collections::HashSet<i32>,
    /// `Raider` descendants — `IS_CELEBRATING` at index 16 BOOLEAN, the same
    /// slot as `DATA_BABY_ID` and `DATA_DANCING` (M20).
    raider: std::collections::HashSet<i32>,
    /// `SpellcasterIllager` descendants — `DATA_SPELL_CASTING_ID` at 17 BYTE.
    spellcaster: std::collections::HashSet<i32>,
    /// `AbstractIllager` descendants — the `IllagerModel` arm-pose switch.
    illager: std::collections::HashSet<i32>,
    /// `Player` descendants — excluded from death entity-event 3's
    /// client-side `setHealth(0) + die()` (M24).
    player: std::collections::HashSet<i32>,
    /// `AbstractMinecart` descendants — `handleMinecartAlongTrack`'s first
    /// `instanceof` (M77).
    minecart: std::collections::HashSet<i32>,
    /// `AbstractHurtingProjectile` descendants —
    /// `handleProjectilePowerPacket`'s only cast (M77).
    hurting_projectile: std::collections::HashSet<i32>,
    /// `Leashable` implementors — `handleEntityLinkPacket`'s only cast (M77).
    /// The one set here derived from an `implements` edge rather than an
    /// `extends` chain; see [`crate::entity_classes::LEASHABLE`].
    leashable: std::collections::HashSet<i32>,
    /// M169 — `instanceof PlayerRideableJumping`, the jump bar's vehicles.
    rideable_jumping: std::collections::HashSet<i32>,
    /// M169 — `Camel`: the 55-tick dash cooldown off index 19, and the
    /// `refuseToMove()` clause in `canJump()`.
    camel: std::collections::HashSet<i32>,
    /// M296 — `AbstractChestedHorse` (donkey, mule, the llamas).
    chested_horse: std::collections::HashSet<i32>,
    /// M169 — `AbstractNautilus`: the 40-tick dash cooldown off index 20.
    nautilus: std::collections::HashSet<i32>,
    /// M194 — `minecraft:item_frame` and `minecraft:glow_item_frame`, the two
    /// `ItemFrame` types: they own metadata slots 8 (DIRECTION), 9
    /// (ITEM_STACK) and 10 (INT).
    item_frame: std::collections::HashSet<i32>,
    /// M259 — `minecraft:evoker_fangs`: entity event 4 is its bite.
    evoker_fangs: std::collections::HashSet<i32>,
    /// M358 — `minecraft:sheep`: entity event 10 is its grazing.
    sheep: std::collections::HashSet<i32>,
    /// M365 — `minecraft:turtle`: index 18 / 19 BOOLEAN are its egg flags.
    turtle: std::collections::HashSet<i32>,
    /// M387 — `minecraft:axolotl`: index 19 BOOLEAN is `DATA_PLAYING_DEAD`.
    axolotl: std::collections::HashSet<i32>,
    /// M389 — `minecraft:piglin`: index 17 BOOLEAN is its `DATA_BABY_ID`,
    /// 18 `DATA_IS_CHARGING_CROSSBOW`, 19 `DATA_IS_DANCING`.
    piglin: std::collections::HashSet<i32>,
    /// M389 — `AbstractPiglin` (the piglin and the brute): index 16 BOOLEAN
    /// is `DATA_IMMUNE_TO_ZOMBIFICATION`, not a baby flag.
    abstract_piglin: std::collections::HashSet<i32>,
    /// M367 — `minecraft:hoglin` and `minecraft:zoglin`: entity event 4 is
    /// their headbutt (`attackAnimationRemainingTicks = 10`).
    hoglin: std::collections::HashSet<i32>,
    /// M219 — `instanceof HasCustomInventoryScreen`: its three implementors'
    /// subtrees (`AbstractHorse`, `AbstractNautilus`, `AbstractChestBoat`).
    /// Riding one turns the inventory key into `OPEN_INVENTORY`.
    custom_inventory: std::collections::HashSet<i32>,
    /// M220 — `instanceof AbstractHorse`: `handleMountScreenOpen`'s first
    /// branch (`HorseInventoryMenu`).
    abstract_horse: std::collections::HashSet<i32>,
    /// M221 — `instanceof AbstractBoat` (boats, rafts, chest boats): the
    /// vehicles the client steers with `AbstractBoat`'s physics.
    abstract_boat: std::collections::HashSet<i32>,
    /// M305 — `instanceof AbstractCubeMob` (slime, magma cube, sulfur cube):
    /// `ID_SIZE` is their index-18 INT.
    cube_mob: std::collections::HashSet<i32>,
}

impl EntityClasses {
    /// Resolve the generated names against the runtime registry.
    ///
    /// Hard-fails on drift: a registry of a different size than the generated
    /// pin, or a generated name the registry does not contain. Either means the
    /// table was built from another version, and a silently missing name would
    /// turn into "this mob is not living" — a whole class of packets quietly
    /// ignored.
    pub fn resolve(types: &EntityTypes) -> Result<Self, String> {
        use crate::entity_classes as table;
        if types.len() != table::REGISTERED_TYPES {
            return Err(format!(
                "entity_classes: the entity_type registry has {} entries but the generated \
                 table was built from {} — re-run tools/gen_entity_classes.py after the \
                 version bump",
                types.len(),
                table::REGISTERED_TYPES
            ));
        }
        let resolve_set =
            |names: &[&str], what: &str| -> Result<std::collections::HashSet<i32>, String> {
                names
                    .iter()
                    .map(|name| {
                        types.id_of(name).ok_or_else(|| {
                            format!(
                                "entity_classes: generated {what} entity {name:?} is not in the \
                             entity_type registry — re-run tools/gen_entity_classes.py"
                            )
                        })
                    })
                    .collect()
            };
        let living = resolve_set(table::LIVING, "living")?;
        let swing_ticking = resolve_set(table::SWING_TICKING, "swing-ticking")?;
        let mob = resolve_set(table::MOB, "mob")?;
        let raider = resolve_set(table::RAIDER, "raider")?;
        let spellcaster = resolve_set(table::SPELLCASTER_ILLAGER, "spellcaster")?;
        let illager = resolve_set(table::ILLAGER, "illager")?;
        let player = resolve_set(table::PLAYER, "player")?;
        let minecart = resolve_set(table::ABSTRACT_MINECART, "minecart")?;
        let hurting_projectile =
            resolve_set(table::ABSTRACT_HURTING_PROJECTILE, "hurting-projectile")?;
        let leashable = resolve_set(table::LEASHABLE, "leashable")?;
        let rideable_jumping = resolve_set(table::PLAYER_RIDEABLE_JUMPING, "rideable-jumping")?;
        let camel = resolve_set(table::CAMEL, "camel")?;
        let chested_horse = resolve_set(table::ABSTRACT_CHESTED_HORSE, "chested-horse")?;
        let nautilus = resolve_set(table::ABSTRACT_NAUTILUS, "nautilus")?;
        let abstract_horse = resolve_set(table::ABSTRACT_HORSE, "horse")?;
        let abstract_boat = resolve_set(table::ABSTRACT_BOAT, "boat")?;
        let cube_mob = resolve_set(table::ABSTRACT_CUBE_MOB, "cube-mob")?;
        let mut custom_inventory = abstract_horse.clone();
        custom_inventory.extend(nautilus.iter().copied());
        custom_inventory.extend(resolve_set(table::ABSTRACT_CHEST_BOAT, "chest-boat")?);
        // A swing-ticking type that is not living would mean the generator's
        // own invariant broke between generation and resolution.
        if let Some(bad) = swing_ticking.iter().find(|id| !living.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} ticks a swing but is not living"
            ));
        }
        log::info!(
            "rewo-data: entity classes — {} living, {} swing-ticking of {} types",
            living.len(),
            swing_ticking.len(),
            types.len()
        );
        // Every illager is a raider, and every spellcaster is an illager —
        // the `extends` chain guarantees it, so a violation means the generated
        // table and the registry disagree about the hierarchy.
        if let Some(bad) = illager.iter().find(|id| !raider.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is an illager but not a raider"
            ));
        }
        if let Some(bad) = spellcaster.iter().find(|id| !illager.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is a spellcaster but not an illager"
            ));
        }
        if let Some(bad) = raider.iter().find(|id| !mob.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is a raider but not a mob"
            ));
        }
        // A `Player` is a `LivingEntity` but never a `Mob` — the Avatar branch
        // of the hierarchy. Both directions are asserted because the death
        // event's exclusion depends on the set being neither empty nor
        // accidentally widened to every humanoid.
        if let Some(bad) = player.iter().find(|id| !living.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is a player but not living"
            ));
        }
        if let Some(bad) = player.iter().find(|id| mob.contains(id)) {
            return Err(format!("entity_classes: type {bad} is a player and a mob"));
        }
        // M77. `Mob implements Leashable`, so every mob is leashable and the
        // set is a strict superset of `mob` — the boats are the difference.
        // Asserting it is what catches an `implements` scan that silently
        // matched nothing: an empty `leashable` would make every
        // `set_entity_link` inert, which reads exactly like a server that
        // never leashes anything.
        if let Some(bad) = mob.iter().find(|id| !leashable.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is a mob but not leashable"
            ));
        }
        if leashable.len() <= mob.len() {
            return Err(format!(
                "entity_classes: {} leashable types against {} mobs — the boats \
                 are missing, so the `implements` scan lost a subtree",
                leashable.len(),
                mob.len()
            ));
        }
        // A hurting projectile is an `Entity`, never a `LivingEntity`: the
        // packet's cast and the living gate must not overlap, or one of the
        // two tables was built from a different hierarchy.
        if let Some(bad) = hurting_projectile.iter().find(|id| living.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is a hurting projectile and living"
            ));
        }
        // M169. The camel and the nautilus pair each arm a dash cooldown the
        // jump bar reads, and both are rideable-jumping; the horses are the
        // rest of that set. All three are living. An `implements` scan that
        // matched nothing would make every saddled horse un-jumpable, which
        // reads exactly like a server that never saddles anything.
        if let Some(bad) = rideable_jumping.iter().find(|id| !living.contains(id)) {
            return Err(format!(
                "entity_classes: type {bad} is rideable-jumping but not living"
            ));
        }
        if let Some(bad) = camel
            .iter()
            .chain(nautilus.iter())
            .find(|id| !rideable_jumping.contains(id))
        {
            return Err(format!(
                "entity_classes: type {bad} dashes but is not rideable-jumping"
            ));
        }
        if rideable_jumping.len() <= camel.len() + nautilus.len() {
            return Err(format!(
                "entity_classes: {} rideable-jumping types against {} camels and {} \
                 nautiluses — the horses are missing, so the `implements` scan lost \
                 a subtree",
                rideable_jumping.len(),
                camel.len(),
                nautilus.len()
            ));
        }
        Ok(Self {
            living,
            swing_ticking,
            mob,
            raider,
            spellcaster,
            illager,
            player,
            minecart,
            hurting_projectile,
            leashable,
            rideable_jumping,
            camel,
            chested_horse,
            nautilus,
            item_frame: ["minecraft:item_frame", "minecraft:glow_item_frame"]
                .iter()
                .filter_map(|n| types.id_of(n))
                .collect(),
            evoker_fangs: types.id_of("minecraft:evoker_fangs").into_iter().collect(),
            sheep: types.id_of("minecraft:sheep").into_iter().collect(),
            turtle: types.id_of("minecraft:turtle").into_iter().collect(),
            axolotl: types.id_of("minecraft:axolotl").into_iter().collect(),
            piglin: types.id_of("minecraft:piglin").into_iter().collect(),
            abstract_piglin: ["minecraft:piglin", "minecraft:piglin_brute"]
                .iter()
                .filter_map(|n| types.id_of(n))
                .collect(),
            hoglin: ["minecraft:hoglin", "minecraft:zoglin"]
                .iter()
                .filter_map(|n| types.id_of(n))
                .collect(),
            custom_inventory,
            abstract_horse,
            abstract_boat,
            cube_mob,
        })
    }

    /// Build a classification directly from ids, for **unit tests** that must
    /// not read the datagen reports.
    ///
    /// Every production path and every oracle uses [`Self::resolve`] against
    /// the real registry — this exists so a codec test in another crate can
    /// exercise the living / ticking gates without a filesystem, and nothing
    /// that ships reads it.
    pub fn from_raw_ids(living: &[i32], swing_ticking: &[i32]) -> Self {
        Self {
            living: living.iter().copied().collect(),
            swing_ticking: swing_ticking.iter().copied().collect(),
            mob: Default::default(),
            raider: Default::default(),
            spellcaster: Default::default(),
            illager: Default::default(),
            player: Default::default(),
            minecart: Default::default(),
            hurting_projectile: Default::default(),
            leashable: Default::default(),
            rideable_jumping: Default::default(),
            camel: Default::default(),
            chested_horse: Default::default(),
            nautilus: Default::default(),
            item_frame: Default::default(),
            evoker_fangs: Default::default(),
            sheep: Default::default(),
            turtle: Default::default(),
            axolotl: Default::default(),
            piglin: Default::default(),
            abstract_piglin: Default::default(),
            hoglin: Default::default(),
            custom_inventory: Default::default(),
            abstract_horse: Default::default(),
            abstract_boat: Default::default(),
            cube_mob: Default::default(),
        }
    }

    /// As [`Self::from_raw_ids`], with the M20 ancestry sets. Tests only.
    pub fn from_raw_ids_full(
        living: &[i32],
        swing_ticking: &[i32],
        mob: &[i32],
        raider: &[i32],
        spellcaster: &[i32],
        illager: &[i32],
    ) -> Self {
        Self {
            living: living.iter().copied().collect(),
            swing_ticking: swing_ticking.iter().copied().collect(),
            mob: mob.iter().copied().collect(),
            raider: raider.iter().copied().collect(),
            spellcaster: spellcaster.iter().copied().collect(),
            illager: illager.iter().copied().collect(),
            player: Default::default(),
            minecart: Default::default(),
            hurting_projectile: Default::default(),
            leashable: Default::default(),
            rideable_jumping: Default::default(),
            camel: Default::default(),
            chested_horse: Default::default(),
            nautilus: Default::default(),
            item_frame: Default::default(),
            evoker_fangs: Default::default(),
            sheep: Default::default(),
            turtle: Default::default(),
            axolotl: Default::default(),
            piglin: Default::default(),
            abstract_piglin: Default::default(),
            hoglin: Default::default(),
            custom_inventory: Default::default(),
            abstract_horse: Default::default(),
            abstract_boat: Default::default(),
            cube_mob: Default::default(),
        }
    }

    /// `entity instanceof Mob` — gates reading the index-15 BYTE as
    /// `DATA_MOB_FLAGS_ID` rather than an armor stand's client flags.
    pub fn is_mob(&self, type_id: i32) -> bool {
        self.mob.contains(&type_id)
    }

    /// `entity instanceof Raider` — decides that index-16 BOOLEAN is
    /// `IS_CELEBRATING` rather than `DATA_BABY_ID`.
    pub fn is_raider(&self, type_id: i32) -> bool {
        self.raider.contains(&type_id)
    }

    /// `entity instanceof SpellcasterIllager` — index-17 BYTE is
    /// `DATA_SPELL_CASTING_ID`.
    pub fn is_spellcaster(&self, type_id: i32) -> bool {
        self.spellcaster.contains(&type_id)
    }

    /// `entity instanceof AbstractIllager` — runs `IllagerModel`'s arm-pose
    /// switch instead of the humanoid/undead rigs.
    pub fn is_illager(&self, type_id: i32) -> bool {
        self.illager.contains(&type_id)
    }

    /// `entity instanceof Player` — the exclusion in death entity-event 3's
    /// `if (!(this instanceof Player)) { setHealth(0); die(...); }`.
    ///
    /// A set rather than an equality against the player type id: `Mannequin`
    /// shares the `Avatar` base and is *not* a `Player`, so the distinction is
    /// a hierarchy question the machine extractor answers.
    pub fn is_player(&self, type_id: i32) -> bool {
        self.player.contains(&type_id)
    }

    /// `entity instanceof LivingEntity`.
    pub fn is_living(&self, type_id: i32) -> bool {
        self.living.contains(&type_id)
    }

    /// `entity instanceof AbstractMinecart` — `handleMinecartAlongTrack`'s
    /// first guard (M77). The second, `getBehavior() instanceof
    /// NewMinecartBehavior`, is a runtime state rather than a class fact and
    /// cannot live here; see [`crate::entity_classes::ABSTRACT_MINECART`] and
    /// the note on `rewo_net::route_move_minecart_along_track`.
    pub fn is_minecart(&self, type_id: i32) -> bool {
        self.minecart.contains(&type_id)
    }

    /// M259 — `minecraft:evoker_fangs`, whose entity event 4 is its bite.
    pub fn is_evoker_fangs(&self, type_id: i32) -> bool {
        self.evoker_fangs.contains(&type_id)
    }

    /// M367 — a hoglin or zoglin (`handleEntityEvent(4)`: the headbutt).
    pub fn is_hoglin(&self, type_id: i32) -> bool {
        self.hoglin.contains(&type_id)
    }

    /// M387 — `minecraft:axolotl` (`DATA_PLAYING_DEAD`, index 19).
    pub fn is_axolotl(&self, type_id: i32) -> bool {
        self.axolotl.contains(&type_id)
    }

    /// M389 — `minecraft:piglin` (baby 17, charging 18, dancing 19).
    pub fn is_piglin(&self, type_id: i32) -> bool {
        self.piglin.contains(&type_id)
    }

    /// M389 — `instanceof AbstractPiglin`: its index 16 BOOLEAN is
    /// `DATA_IMMUNE_TO_ZOMBIFICATION`.
    pub fn is_abstract_piglin(&self, type_id: i32) -> bool {
        self.abstract_piglin.contains(&type_id)
    }

    /// M365 — `minecraft:turtle` (`Turtle.HAS_EGG` 18, `LAYING_EGG` 19).
    pub fn is_turtle(&self, type_id: i32) -> bool {
        self.turtle.contains(&type_id)
    }

    /// M358 — `minecraft:sheep`, whose entity event 10 is its grazing.
    pub fn is_sheep(&self, type_id: i32) -> bool {
        self.sheep.contains(&type_id)
    }

    /// M194 — an `ItemFrame` (plain or glow).
    pub fn is_item_frame(&self, type_id: i32) -> bool {
        self.item_frame.contains(&type_id)
    }

    /// `entity instanceof AbstractHurtingProjectile` —
    /// `handleProjectilePowerPacket`'s only cast (M77). An arrow is an
    /// `AbstractArrow` and fails this, so a `projectile_power` naming one
    /// mutates nothing.
    pub fn is_hurting_projectile(&self, type_id: i32) -> bool {
        self.hurting_projectile.contains(&type_id)
    }

    /// `entity instanceof Leashable` — `handleEntityLinkPacket`'s only cast
    /// (M77). An interface, so this is the union of `Mob`'s and
    /// `AbstractBoat`'s subtrees rather than one ancestry walk.
    pub fn is_leashable(&self, type_id: i32) -> bool {
        self.leashable.contains(&type_id)
    }

    /// `entity instanceof PlayerRideableJumping` (M169) — what
    /// `LocalPlayer.jumpableVehicle()` casts to. The horse family and the
    /// nautilus pair; a union of two subtrees, like `is_leashable`.
    pub fn is_rideable_jumping(&self, type_id: i32) -> bool {
        self.rideable_jumping.contains(&type_id)
    }

    /// `entity instanceof AbstractBoat` (M221).
    pub fn is_boat(&self, type_id: i32) -> bool {
        self.abstract_boat.contains(&type_id)
    }

    /// `entity instanceof AbstractCubeMob` (M305): index 18 INT is its size.
    pub fn is_cube_mob(&self, type_id: i32) -> bool {
        self.cube_mob.contains(&type_id)
    }

    /// As built, plus these ids as cube mobs. **Unit tests only**, beside
    /// [`Self::from_raw_ids`].
    pub fn with_cube_mobs(mut self, ids: &[i32]) -> Self {
        self.cube_mob.extend(ids.iter().copied());
        self
    }

    /// As built, plus `piglin` as the piglin and `brute` as the brute (both
    /// `AbstractPiglin`). **Unit tests only**, beside [`Self::from_raw_ids`].
    pub fn with_piglins(mut self, piglin: &[i32], brute: &[i32]) -> Self {
        self.piglin.extend(piglin.iter().copied());
        self.abstract_piglin
            .extend(piglin.iter().chain(brute).copied());
        self
    }

    /// `entity instanceof AbstractHorse` (M220).
    pub fn is_abstract_horse(&self, type_id: i32) -> bool {
        self.abstract_horse.contains(&type_id)
    }

    /// `entity instanceof HasCustomInventoryScreen` (M219).
    pub fn has_custom_inventory(&self, type_id: i32) -> bool {
        self.custom_inventory.contains(&type_id)
    }

    /// `entity instanceof AbstractChestedHorse` (M296).
    pub fn is_chested_horse(&self, type_id: i32) -> bool {
        self.chested_horse.contains(&type_id)
    }

    /// `entity instanceof Camel` (M169).
    pub fn is_camel(&self, type_id: i32) -> bool {
        self.camel.contains(&type_id)
    }

    /// `entity instanceof AbstractNautilus` (M169).
    pub fn is_nautilus(&self, type_id: i32) -> bool {
        self.nautilus.contains(&type_id)
    }

    /// Whether this type's client class advances `updateSwingTime` each tick.
    pub fn ticks_swing(&self, type_id: i32) -> bool {
        self.swing_ticking.contains(&type_id)
    }

    pub fn living_count(&self) -> usize {
        self.living.len()
    }

    pub fn swing_ticking_count(&self) -> usize {
        self.swing_ticking.len()
    }
}
