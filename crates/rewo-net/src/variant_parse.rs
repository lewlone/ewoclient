//! The `minecraft:{cat,wolf,frog}_variant` registries, out of Configuration's
//! `registry_data` (M64).
//!
//! In 26.x these three mobs stopped carrying an int ordinal and started
//! carrying a `Holder` — `EntityDataSerializers.{CAT,WOLF,FROG}_VARIANT`, all
//! of them `ByteBufCodecs.holderRegistry(...)`, which writes a **raw 0-based
//! registry id** (not `holder`'s `id + 1` inline scheme; that distinction is
//! REWO_PLAN §0.0's, and M16 records the same trap for dimension types).
//!
//! They are **datapack** registries, so their contents *and their id order*
//! are the server's — nothing here may be derived from Mojang's bootstrap
//! order. `RegistryDataLoader.SYNCHRONIZED_REGISTRIES` lists all three with a
//! `NETWORK_CODEC` that carries the asset ids, which is exactly what the
//! client needs and why the join downstream is on the *texture*, never on the
//! id.
//!
//! # The two entry shapes
//!
//! ```text
//! cat_variant / frog_variant     wolf_variant
//!   asset_id:      <Identifier>    assets:      { wild, tame, angry }
//!   baby_asset_id: <Identifier>    baby_assets: { wild, tame, angry }
//! ```
//!
//! `CatVariant`/`FrogVariant` use `ClientAsset.ResourceTexture.
//! DEFAULT_FIELD_CODEC`, which is `CODEC.fieldOf("asset_id")` — so the field
//! name is `asset_id` even though the record member is `adultAssetInfo`.
//! `WolfVariant` nests an `AssetInfo` of three under `assets`.
//!
//! Both are read here as **texture paths**, through
//! `ClientAsset.ResourceTexture`'s own expansion
//! `id.withPath(p -> "textures/" + p + ".png")`, minus the `textures/` prefix
//! Rewo's mob-texture specs drop as well. A datapack that names a texture the
//! jar does not ship simply fails to resolve downstream and the mob keeps its
//! base sheet — the M57b rule, and the opposite of inventing one.

use rewo_proto::nbt::Nbt;

pub const CAT_VARIANT_REGISTRY: &str = "minecraft:cat_variant";
pub const WOLF_VARIANT_REGISTRY: &str = "minecraft:wolf_variant";
pub const FROG_VARIANT_REGISTRY: &str = "minecraft:frog_variant";

/// One variant entry, at the index that is its protocol id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MobVariantDef {
    /// `minecraft:jellie`.
    pub id: String,
    /// The adult sheet — `asset_id` (cat, frog) or `assets.wild` (wolf), as a
    /// jar-relative texture path.
    pub wild: Option<String>,
    /// `assets.tame`, wolves only.
    pub tame: Option<String>,
    /// `assets.angry`, wolves only. **Decoded and not rendered**: choosing it
    /// needs `remainingPersistentAngerTime > 0`, i.e. `DATA_ANGER_END_TIME`
    /// (index 22, LONG) against the world clock, which makes it a texture that
    /// changes with time rather than with a synched value. Kept so the gap is
    /// visible in the parsed data rather than only in a comment.
    pub angry: Option<String>,
}

impl MobVariantDef {
    /// `Wolf.getTexture`'s tame/wild branch, minus the angry one.
    pub fn texture(&self, tame: bool) -> Option<&str> {
        if tame {
            self.tame.as_deref().or(self.wild.as_deref())
        } else {
            self.wild.as_deref()
        }
    }
}

/// `ClientAsset.ResourceTexture(id)` — `id.withPath(p -> "textures/" + p +
/// ".png")`, with the namespace and the `textures/` prefix dropped so it lines
/// up with `rewo_data::assets`'s jar-relative mob-texture paths.
fn texture_path(asset_id: &str) -> String {
    let path = asset_id.split_once(':').map_or(asset_id, |(_, p)| p);
    format!("{path}.png")
}

fn string_at(nbt: Option<&Nbt>, key: &str) -> Option<String> {
    match nbt?.get(key)? {
        Nbt::String(s) => Some(texture_path(s)),
        _ => None,
    }
}

fn read_entries<T>(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
    mut f: impl FnMut(String, Option<Nbt>) -> T,
) -> Vec<T> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Ok(id) = r.identifier() else { break };
        // `Optional<NBT>` — present only because Rewo answers
        // `select_known_packs` with an empty list.
        let data = match r.u8() {
            Ok(0) => None,
            Ok(_) => r.nbt().ok(),
            Err(_) => break,
        };
        out.push(f(id, data));
    }
    out
}

/// `cat_variant` / `frog_variant`: one `asset_id` per entry.
pub fn parse_single_asset_registry(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
) -> Vec<MobVariantDef> {
    read_entries(r, count, |id, data| MobVariantDef {
        id,
        wild: string_at(data.as_ref(), "asset_id"),
        tame: None,
        angry: None,
    })
}

/// `wolf_variant`: a nested `assets { wild, tame, angry }`.
pub fn parse_wolf_variant_registry(
    r: &mut rewo_proto::reader::PacketReader,
    count: usize,
) -> Vec<MobVariantDef> {
    read_entries(r, count, |id, data| {
        let assets = data.as_ref().and_then(|n| n.get("assets")).cloned();
        MobVariantDef {
            id,
            wild: string_at(assets.as_ref(), "wild"),
            tame: string_at(assets.as_ref(), "tame"),
            angry: string_at(assets.as_ref(), "angry"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::reader::PacketReader;

    /// `<identifier> <has-data> <nbt>` per entry, as `registry_data` writes
    /// it. The NBT encoder is `dimension_parse`'s fixture writer — one
    /// encoder for every registry fixture, so a shape mismatch shows up in
    /// all of them at once rather than in whichever one has its own.
    fn body(entries: &[(&str, Option<Nbt>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, nbt) in entries {
            let mut w = rewo_proto::writer::PacketWriter::default();
            w.string(name).bool(nbt.is_some());
            out.extend_from_slice(&w.buf);
            if let Some(n) = nbt {
                crate::dimension_parse::builtin::write_network_nbt(&mut out, n);
            }
        }
        out
    }

    fn compound(pairs: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        )
    }

    fn s(v: &str) -> Nbt {
        Nbt::String(v.to_string())
    }

    #[test]
    fn a_cat_entry_expands_its_asset_id_into_a_texture_path() {
        let b = body(&[(
            "minecraft:jellie",
            Some(compound(&[
                ("asset_id", s("minecraft:entity/cat/cat_jellie")),
                ("baby_asset_id", s("minecraft:entity/cat/cat_jellie_baby")),
            ])),
        )]);
        let v = parse_single_asset_registry(&mut PacketReader::new(&b), 1);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "minecraft:jellie");
        // `ResourceTexture` prepends `textures/` and appends `.png`; Rewo drops
        // the prefix, so the adult sheet is the jar-relative path.
        assert_eq!(v[0].wild.as_deref(), Some("entity/cat/cat_jellie.png"));
        assert_eq!(v[0].tame, None, "a cat has no tame sheet");
    }

    #[test]
    fn a_wolf_entry_reads_all_three_nested_assets() {
        let b = body(&[(
            "minecraft:ashen",
            Some(compound(&[(
                "assets",
                compound(&[
                    ("wild", s("minecraft:entity/wolf/wolf_ashen")),
                    ("tame", s("minecraft:entity/wolf/wolf_ashen_tame")),
                    ("angry", s("minecraft:entity/wolf/wolf_ashen_angry")),
                ]),
            )])),
        )]);
        let v = parse_wolf_variant_registry(&mut PacketReader::new(&b), 1);
        assert_eq!(v[0].wild.as_deref(), Some("entity/wolf/wolf_ashen.png"));
        assert_eq!(v[0].tame.as_deref(), Some("entity/wolf/wolf_ashen_tame.png"));
        assert_eq!(
            v[0].angry.as_deref(),
            Some("entity/wolf/wolf_ashen_angry.png"),
            "decoded even though nothing renders it"
        );
        // `Wolf.getTexture`'s branch.
        assert_eq!(v[0].texture(false), Some("entity/wolf/wolf_ashen.png"));
        assert_eq!(v[0].texture(true), Some("entity/wolf/wolf_ashen_tame.png"));
    }

    /// Wire order is the id order, and an entry that carries no data at all
    /// is kept in place rather than dropped — otherwise every later entry's
    /// id would shift by one.
    #[test]
    fn the_index_is_the_protocol_id_even_when_an_entry_is_empty() {
        let b = body(&[
            ("minecraft:cold", None),
            (
                "minecraft:warm",
                Some(compound(&[("asset_id", s("minecraft:entity/frog/frog_warm"))])),
            ),
        ]);
        let v = parse_single_asset_registry(&mut PacketReader::new(&b), 2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "minecraft:cold");
        assert_eq!(v[0].wild, None);
        assert_eq!(v[1].wild.as_deref(), Some("entity/frog/frog_warm.png"));
    }
}
