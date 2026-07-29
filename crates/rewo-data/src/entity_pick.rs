//! The per-entity-type half of the crosshair entity pick (M73): the bounding
//! box a ray is swept against, and `Entity.isPickable()`.
//!
//! Both come from [`crate::entity_pick_table`], machine-extracted by
//! `tools/gen_entity_pick.py`, resolved here against the live registry exactly
//! as [`crate::entity_types::EntityClasses`] resolves its name tables — a
//! registry of a different size, or a generated name the registry does not
//! carry, is a hard error rather than a quietly missing entry.
//!
//! **`isPickable()` defaults to `false`.** That is the fact worth stating up
//! front, because it inverts the intuition: the pick's job is not to find
//! reasons to exclude an entity, it is that only the thirteen classes that
//! override the method are pickable at all. A dropped item, an experience orb,
//! an armour-stand-shaped `marker`, a `text_display` and a `lightning_bolt` are
//! all invisible to the crosshair — and so is the **ender dragon**, which
//! overrides it back to `false` and delegates to its `EnderDragonPart`
//! hitboxes, none of which is a registered entity type.
//!
//! The one rule this table cannot answer alone is
//! [`PickRule::RedirectableProjectile`], because `Projectile.isPickable()` is
//! `this.is(EntityTypeTags.REDIRECTABLE_PROJECTILE)` — a **tag**, which lives
//! in the client jar's data pack rather than in any class. It is read as a tag
//! by [`EntityTypeTag::load_redirectable_projectile`], for the same reason M19
//! reads `ItemTags.SPEARS` as a tag rather than inferring it from a component:
//! answering a data question with a code question is right only by coincidence.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

pub use crate::entity_pick_table::PickRule;

use crate::entity_types::EntityTypes;

/// `data/minecraft/tags/entity_type/redirectable_projectile.json` inside the
/// client jar.
const REDIRECTABLE_TAG_PATH: &str = "data/minecraft/tags/entity_type/redirectable_projectile.json";

/// One entity's static pick inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickShape {
    /// `EntityType.Builder.sized(w, _)`.
    pub width: f32,
    /// `EntityType.Builder.sized(_, h)`.
    pub height: f32,
    /// Which `isPickable()` body this type inherits.
    pub rule: PickRule,
}

/// Every registered type's dimensions and pick rule, keyed by protocol id.
#[derive(Clone, Debug)]
pub struct EntityPickTable {
    by_id: HashMap<i32, PickShape>,
}

impl EntityPickTable {
    /// Resolve the generated table against the runtime registry.
    pub fn resolve(types: &EntityTypes) -> Result<Self, String> {
        use crate::entity_pick_table as table;
        if types.len() != table::REGISTERED_TYPES {
            return Err(format!(
                "entity_pick: the entity_type registry has {} entries but the generated \
                 table was built from {} — re-run tools/gen_entity_pick.py after the \
                 version bump",
                types.len(),
                table::REGISTERED_TYPES
            ));
        }
        let mut by_id = HashMap::with_capacity(table::ENTITY_PICK.len());
        for (name, width, height, rule) in table::ENTITY_PICK {
            let id = types.id_of(name).ok_or_else(|| {
                format!(
                    "entity_pick: generated entity {name:?} is not in the entity_type \
                     registry — re-run tools/gen_entity_pick.py"
                )
            })?;
            by_id.insert(
                id,
                PickShape {
                    width: *width,
                    height: *height,
                    rule: *rule,
                },
            );
        }
        // Every registered id must be covered, or a type the server can spawn
        // would fall through to "no shape" and be silently unpickable.
        if let Some(missing) = types.ids().find(|id| !by_id.contains_key(id)) {
            return Err(format!(
                "entity_pick: registry type {missing} ({:?}) has no generated row",
                types.name(missing)
            ));
        }
        log::info!(
            "rewo-data: entity pick shapes — {} types, {} never pickable",
            by_id.len(),
            by_id
                .values()
                .filter(|s| s.rule == PickRule::Never)
                .count()
        );
        Ok(Self { by_id })
    }

    /// This type's dimensions and rule, or `None` for an id the registry does
    /// not carry.
    pub fn get(&self, type_id: i32) -> Option<PickShape> {
        self.by_id.get(&type_id).copied()
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// A resolved entity-type tag, keyed by protocol id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityTypeTag {
    ids: HashSet<i32>,
}

impl EntityTypeTag {
    pub fn contains(&self, type_id: i32) -> bool {
        self.ids.contains(&type_id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Load `minecraft:redirectable_projectile` from the client jar.
    ///
    /// **Fails loud on every unrecognised form** rather than dropping entries,
    /// exactly as `ItemTag::load_spears` does: a missing tag file, an empty
    /// value list, a `#other_tag` reference, an object-form entry, a non-string
    /// value, or a name absent from the entity registry. A silently shrunk set
    /// makes a redirectable projectile unpickable, which looks like the correct
    /// default and is not.
    pub fn load_redirectable_projectile(
        client_jar: &Path,
        types: &EntityTypes,
    ) -> Result<Self, String> {
        let file = std::fs::File::open(client_jar)
            .map_err(|e| format!("open {}: {e}", client_jar.display()))?;
        let mut jar = zip::ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| format!("zip {}: {e}", client_jar.display()))?;
        let mut text = String::new();
        jar.by_name(REDIRECTABLE_TAG_PATH)
            .map_err(|e| format!("{REDIRECTABLE_TAG_PATH}: {e}"))?
            .read_to_string(&mut text)
            .map_err(|e| format!("{REDIRECTABLE_TAG_PATH}: {e}"))?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("{REDIRECTABLE_TAG_PATH}: {e}"))?;
        let values = json
            .get("values")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{REDIRECTABLE_TAG_PATH}: no `values` array"))?;
        if values.is_empty() {
            return Err(format!("{REDIRECTABLE_TAG_PATH}: empty `values`"));
        }
        let mut ids = HashSet::with_capacity(values.len());
        for v in values {
            let name = v
                .as_str()
                .ok_or_else(|| format!("{REDIRECTABLE_TAG_PATH}: non-string entry {v}"))?;
            if let Some(tag) = name.strip_prefix('#') {
                return Err(format!(
                    "{REDIRECTABLE_TAG_PATH}: references tag #{tag}, which this reader \
                     does not expand"
                ));
            }
            let id = types.id_of(name).ok_or_else(|| {
                format!("{REDIRECTABLE_TAG_PATH}: {name:?} is not a registered entity type")
            })?;
            ids.insert(id);
        }
        log::info!(
            "rewo-data: redirectable_projectile tag — {} entity types",
            ids.len()
        );
        Ok(Self { ids })
    }

    /// Build one directly, for tests and oracles that have no jar.
    pub fn from_ids(ids: impl IntoIterator<Item = i32>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::entity_pick_table::{ENTITY_PICK, PickRule, REGISTERED_TYPES};

    #[test]
    fn the_generated_table_covers_every_pinned_type_exactly_once() {
        assert_eq!(ENTITY_PICK.len(), REGISTERED_TYPES);
        let mut names: Vec<&str> = ENTITY_PICK.iter().map(|(n, ..)| *n).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a registry name appears twice");
    }

    fn rule_of(name: &str) -> PickRule {
        ENTITY_PICK
            .iter()
            .find(|(n, ..)| *n == name)
            .unwrap_or_else(|| panic!("{name} missing from the generated table"))
            .3
    }

    fn size_of(name: &str) -> (f32, f32) {
        let row = ENTITY_PICK
            .iter()
            .find(|(n, ..)| *n == name)
            .unwrap_or_else(|| panic!("{name} missing from the generated table"));
        (row.1, row.2)
    }

    #[test]
    fn the_default_is_never_pickable() {
        // `Entity.isPickable()` returns false, so everything that never
        // overrode it is invisible to the crosshair. A dropped stack is the
        // case a reader is most likely to assume the other way round.
        assert_eq!(rule_of("minecraft:item"), PickRule::Never);
        assert_eq!(rule_of("minecraft:experience_orb"), PickRule::Never);
        assert_eq!(rule_of("minecraft:text_display"), PickRule::Never);
        assert_eq!(rule_of("minecraft:marker"), PickRule::Never);
    }

    #[test]
    fn the_ender_dragon_is_not_pickable_but_a_zombie_is() {
        // `EnderDragon.isPickable()` overrides `LivingEntity`'s back to false;
        // only its unregistered `EnderDragonPart` hitboxes are pickable.
        assert_eq!(rule_of("minecraft:ender_dragon"), PickRule::Never);
        assert_eq!(rule_of("minecraft:zombie"), PickRule::Alive);
    }

    #[test]
    fn the_two_conditional_living_rules_are_on_the_right_types() {
        assert_eq!(rule_of("minecraft:player"), PickRule::AliveUnlessSpectator);
        assert_eq!(rule_of("minecraft:armor_stand"), PickRule::AliveUnlessMarker);
    }

    #[test]
    fn an_arrow_needs_the_tag_and_flight_where_a_fireball_needs_only_the_tag() {
        assert_eq!(
            rule_of("minecraft:arrow"),
            PickRule::RedirectableProjectileNotInGround
        );
        assert_eq!(rule_of("minecraft:fireball"), PickRule::RedirectableProjectile);
    }

    #[test]
    fn the_dimensions_are_the_sized_arguments_not_a_humanoid_default() {
        // The pre-M73 hand-written table defaulted everything it did not name
        // to (0.6, 1.8); these three all differ from that and from each other.
        assert_eq!(size_of("minecraft:zombie"), (0.6, 1.95));
        assert_eq!(size_of("minecraft:cow"), (0.9, 1.4));
        assert_eq!(size_of("minecraft:ender_dragon"), (16.0, 8.0));
        assert_eq!(size_of("minecraft:player"), (0.6, 1.8));
    }
}
