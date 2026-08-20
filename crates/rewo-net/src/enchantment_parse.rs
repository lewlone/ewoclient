//! The `minecraft:enchantment` registry, out of Configuration's
//! `registry_data` (M42).
//!
//! M41 decoded the component patch, so an enchanted stack yields
//! `(registry id, level)` pairs — and nothing to translate them with. This is
//! that missing half, and it **has** to come from the wire: enchantments are a
//! **datapack** registry, so both their contents and their id order are
//! whatever the server's packs say. Deriving an id from bootstrap order would
//! be exactly the mistake REWO_PLAN §0.0 gotcha 5 warns about, one registry
//! further down.
//!
//! Two fields matter, and both are in the entry compound:
//!
//! ```text
//! description: <chat component>   // ComponentSerialization.CODEC
//! max_level:   Int                // EnchantmentDefinition's MapCodec is
//!                                 // *inlined*, so this is a top-level field
//!                                 // and not nested under "definition"
//! ```
//!
//! The entry data is only present because Rewo answers `select_known_packs`
//! with an empty list — "I have nothing cached, send everything". A client
//! that claimed the vanilla pack would get names and no payloads, which is why
//! [`EnchantmentDef::description_key`] falls back to the id-derived key rather
//! than requiring the compound.

use rewo_proto::nbt::Nbt;

pub const ENCHANTMENT_REGISTRY: &str = "minecraft:enchantment";

/// One registry entry, at the index that is its protocol id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnchantmentDef {
    /// `minecraft:sharpness`.
    pub id: String,
    /// The translation key of the entry's `description`, or its literal text.
    ///
    /// Kept as the key rather than resolved here because the strings live in
    /// the client jar and this crate has no business reading assets — the
    /// caller pairs it with [`rewo_data::enchantments::EnchantmentText`].
    pub description_key: String,
    /// Whether `description_key` is a literal string rather than a key. A
    /// datapack may name an enchantment with plain text, and translating that
    /// would find nothing and drop the line.
    pub literal: bool,
    /// `EnchantmentDefinition.max_level`, `1..=255`.
    ///
    /// Load-bearing for one rule and nothing else: `getFullname` appends the
    /// level numeral only when `level != 1 || maxLevel != 1`, so a level-1
    /// Mending (max 1) shows no "I" while a level-1 Sharpness (max 5) does.
    pub max_level: i32,
}

impl EnchantmentDef {
    /// `Util.makeDescriptionId("enchantment", id)` — the key vanilla's own
    /// enchantments use, for an entry that arrived without its payload.
    fn derived_key(id: &str) -> String {
        match id.split_once(':') {
            Some((ns, path)) => format!("enchantment.{ns}.{}", path.replace('/', ".")),
            None => format!("enchantment.minecraft.{id}"),
        }
    }
}

/// Parse the enchantment registry's entries, in wire order.
///
/// The vector index **is** the protocol id — the same rule M16 records for
/// dimension types, and the reason nothing here selects by name.
///
/// Tolerant by design: a malformed entry keeps its slot with a derived key and
/// `max_level` 1, because losing the *position* would misname every
/// enchantment after it, which is far worse than one wrong tooltip line.
pub fn parse_enchantment_registry(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
) -> Vec<EnchantmentDef> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Ok(id) = r.identifier() else {
            break;
        };
        // `Optional<NBT>` — present only when the server sent the payload.
        let data = match r.u8() {
            Ok(0) => None,
            Ok(_) => r.nbt().ok(),
            Err(_) => break,
        };
        let (description_key, literal) = match data.as_ref().and_then(|n| n.get("description")) {
            Some(Nbt::String(s)) => (s.clone(), true),
            Some(tag) => match tag.get("translate") {
                Some(Nbt::String(k)) => (k.clone(), false),
                // A component with no `translate` is a literal built from
                // `text` plus children; flattening it is the honest read.
                _ => (nbt_plain(tag), true),
            },
            None => (EnchantmentDef::derived_key(&id), false),
        };
        let max_level = data
            .as_ref()
            .and_then(|n| n.get("max_level"))
            .and_then(|v| v.as_i64())
            .map(|v| v.clamp(1, 255) as i32)
            .unwrap_or(1);
        out.push(EnchantmentDef {
            id,
            description_key,
            literal,
            max_level,
        });
    }
    out
}

/// The literal text of a chat component tag — `text` plus any `extra`.
///
/// **The fourth flattener in the tree**, and deliberately not replaced by
/// [`rewo_world::chat_style::flatten`]: this is the arm for a component with
/// **no** `translate`, reached only after the caller has already split
/// `(description_key, literal)` — so it is asking for the literal text of
/// something already known not to be a key. That split is the structurally
/// right shape and predates M163; the resolution happens at the renderer.
/// See the differential test in [`crate::component_wire`].
pub(crate) fn nbt_plain(tag: &Nbt) -> String {
    let mut out = String::new();
    if let Some(Nbt::String(s)) = tag.get("text") {
        out.push_str(s);
    }
    if let Some(Nbt::List(children)) = tag.get("extra") {
        for child in children {
            out.push_str(&nbt_plain(child));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_payload_falls_back_to_the_derived_key() {
        assert_eq!(
            EnchantmentDef::derived_key("minecraft:sharpness"),
            "enchantment.minecraft.sharpness"
        );
        // A namespaced path with a slash becomes a dot, matching
        // `makeDescriptionId`.
        assert_eq!(
            EnchantmentDef::derived_key("somepack:tier/two"),
            "enchantment.somepack.tier.two"
        );
    }
}
