//! `registries.json` → the `minecraft:attribute` id table, joined against the
//! machine-extracted [`crate::entity_attributes`] tables (M55).
//!
//! **Why the ids are not read off the wire.** `Attribute.STREAM_CODEC` is
//! `ByteBufCodecs.holderRegistry(Registries.ATTRIBUTE)`, and `ATTRIBUTE` is a
//! `BuiltInRegistries` entry — a *bootstrap* registry, not a datapack one. A
//! vanilla server syncs only datapack registries in the Configuration
//! `registry_data` packet, so the ids an `update_attributes` packet carries are
//! the ones the pinned version's own registry assigns. Same rule as
//! [`crate::entity_types`] and [`crate::block_entity_types`], and unlike the
//! enchantment registry M42 reads from the wire: resolved **by name**, so a
//! renumber between versions fails loud instead of silently clamping health to
//! some other attribute's range.
//!
//! The join is the version guard. `registries.json` and the decompiled
//! `Attributes.java` are two independent products of the same jar; if they
//! disagree on the attribute set, one of them was regenerated and the other
//! was not, and [`AttributeRegistry::load`] refuses to build.

use std::collections::HashMap;
use std::path::Path;

use crate::entity_attributes::{AttrDef, ATTRIBUTES, ENTITY_DEFAULTS};
use crate::read_json_file;

/// The `minecraft:attribute` registry, keyed both ways, plus the per-entity
/// default suppliers.
pub struct AttributeRegistry {
    /// Protocol id → definition. Dense: the registry is contiguous from 0.
    by_id: Vec<&'static AttrDef>,
    /// Registry name (without `minecraft:`) → protocol id.
    ids: HashMap<&'static str, i32>,
    /// Entity registry name (with `minecraft:`) → its supplier's base values.
    defaults: HashMap<&'static str, &'static [(&'static str, f64)]>,
}

impl AttributeRegistry {
    /// Build from the datagen report, cross-checking it against the extracted
    /// tables.
    ///
    /// Fails when either side names an attribute the other does not, when a
    /// protocol id is missing, negative, or duplicated, or when the id space is
    /// not the contiguous `0..n` the `holderRegistry` codec's
    /// `byIdOrThrow(VarInt)` assumes.
    pub fn load(path: &Path) -> Result<Self, String> {
        let json = read_json_file(path)?;
        let entries = json
            .get("minecraft:attribute")
            .and_then(|r| r.get("entries"))
            .and_then(|e| e.as_object())
            .ok_or("registries.json: no minecraft:attribute registry")?;

        let defs: HashMap<&'static str, &'static AttrDef> =
            ATTRIBUTES.iter().map(|d| (d.name, d)).collect();

        let mut slots: Vec<Option<&'static AttrDef>> = vec![None; entries.len()];
        let mut ids = HashMap::with_capacity(entries.len());
        for (full, entry) in entries {
            let name = full.strip_prefix("minecraft:").unwrap_or(full);
            let def = *defs.get(name).ok_or_else(|| {
                format!(
                    "registries.json names attribute {full}, which \
                     entity_attributes.rs does not — re-run \
                     tools/gen_entity_attributes.py"
                )
            })?;
            let id = entry
                .get("protocol_id")
                .and_then(|i| i.as_i64())
                .ok_or_else(|| format!("registries.json: {full} has no protocol_id"))?;
            let slot = usize::try_from(id).map_err(|_| {
                format!("registries.json: {full} has negative protocol_id {id}")
            })?;
            let cell = slots.get_mut(slot).ok_or_else(|| {
                format!(
                    "registries.json: {full} has protocol_id {id} outside the \
                     {} entries — the attribute id space is not contiguous",
                    entries.len()
                )
            })?;
            if cell.is_some() {
                return Err(format!("registries.json: duplicate attribute id {id}"));
            }
            *cell = Some(def);
            ids.insert(def.name, slot as i32);
        }

        let by_id = slots
            .into_iter()
            .enumerate()
            .map(|(i, d)| d.ok_or_else(|| format!("registries.json: no attribute at id {i}")))
            .collect::<Result<Vec<_>, _>>()?;

        // The other direction: an attribute the extractor found but the report
        // has lost means the decompile and the report came from different jars.
        if by_id.len() != ATTRIBUTES.len() {
            return Err(format!(
                "registries.json has {} attributes, entity_attributes.rs has {} \
                 — the decompile and the datagen report have drifted",
                by_id.len(),
                ATTRIBUTES.len()
            ));
        }

        log::info!("rewo-data: {} attribute(s)", by_id.len());
        Ok(Self {
            by_id,
            ids,
            defaults: ENTITY_DEFAULTS.iter().map(|(n, d)| (*n, *d)).collect(),
        })
    }

    /// The definition behind a wire id, or `None` when the id is out of range.
    ///
    /// Vanilla's `byIdOrThrow` would throw; a client that drops the snapshot is
    /// strictly better behaved, and the caller cannot continue reading the
    /// packet either way because the id is the first field of the snapshot.
    pub fn def(&self, protocol_id: i32) -> Option<&'static AttrDef> {
        usize::try_from(protocol_id)
            .ok()
            .and_then(|i| self.by_id.get(i))
            .copied()
    }

    /// The wire id of an attribute named without the `minecraft:` prefix.
    pub fn id_of(&self, name: &str) -> Option<i32> {
        self.ids.get(name).copied()
    }

    /// The `AttributeSupplier` base values for an entity type, named with the
    /// `minecraft:` prefix.
    ///
    /// `None` means `DefaultAttributes.SUPPLIERS` has no entry — the type is
    /// not a `LivingEntity`, so it holds no attributes at all and every
    /// resolution against it must fail rather than fall back to a default.
    pub fn defaults_for(&self, entity_name: &str) -> Option<&'static [(&'static str, f64)]> {
        self.defaults.get(entity_name).copied()
    }

    /// The supplier's base value for one attribute of one entity type.
    ///
    /// `None` when the type has no supplier **or** its supplier does not
    /// declare the attribute — the two cases `AttributeMap.getInstance`
    /// collapses into its null return, which `handleUpdateAttributes` logs as
    /// `"Entity {} does not have attribute {}"` and skips.
    pub fn default_base(&self, entity_name: &str, attr: &str) -> Option<f64> {
        self.defaults_for(entity_name)?
            .iter()
            .find(|(n, _)| *n == attr)
            .map(|(_, v)| *v)
    }

    /// Number of registered attributes.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the registry is empty — never true for a successful [`load`].
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}
