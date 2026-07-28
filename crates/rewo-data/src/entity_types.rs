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
        let json = read_json_file(path)?;
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
        log::info!("rewo-data: {} entity types (player = {player_id})", by_id.len());
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
            "item", "experience_orb", "arrow", "spectral_arrow", "trident",
            "snowball", "egg", "ender_pearl", "eye_of_ender", "potion",
            "experience_bottle", "fireball", "small_fireball", "dragon_fireball",
            "wither_skull", "wind_charge", "breeze_wind_charge", "llama_spit",
            "shulker_bullet", "fishing_bobber", "firework_rocket", "tnt",
            "falling_block", "lightning_bolt", "area_effect_cloud", "painting",
            "item_frame", "glow_item_frame", "leash_knot", "marker",
            "interaction", "text_display", "block_display", "item_display",
            "armor_stand", "end_crystal", "evoker_fangs", "ominous_item_spawner",
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
        let resolve_set = |names: &[&str], what: &str| -> Result<std::collections::HashSet<i32>, String> {
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
            return Err(format!(
                "entity_classes: type {bad} is a player and a mob"
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
