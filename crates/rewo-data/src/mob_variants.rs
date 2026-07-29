//! Vanilla's metadata-driven mob texture variants (M64).
//!
//! Seven mobs pick their texture from synched metadata rather than wearing one
//! fixed sheet, and until M64 Rewo baked exactly one of each: every cat was a
//! tabby, every horse brown, every axolotl leucistic. The *rendering* half was
//! already built — M57b's ETF alternates pack a same-sized texture elsewhere in
//! the entity atlas and address it by a per-draw variant id, and that is
//! precisely the shape this needs. What was missing was the textures, the
//! id → texture mapping, and the metadata decode ([`crate::mob_variant_ids`]
//! has none of those; they live in `rewo-net` and `live_cmd`).
//!
//! # Two kinds of variant, and only one of them is a constant
//!
//! **Horse, llama and axolotl** carry an `int` whose value is an enum ordinal,
//! so their mapping is a transcribed table and is fixed for the version.
//!
//! **Cat, wolf and frog** moved to *datapack registries* in 26.x
//! (`Holder<CatVariant>` and friends, over `ByteBufCodecs.holderRegistry`, so
//! the wire value is a raw 0-based registry id). Their contents *and their id
//! order* are the server's, and they arrive in Configuration's `registry_data`
//! carrying the texture each entry names — REWO_PLAN §0.0's rule, the same one
//! M16 records for dimension types and M42 for enchantments. So the join here
//! is the **texture path**, never the id: `rewo-net` reads the registry, this
//! module turns the path it names into a local variant id, and nothing depends
//! on the server having registered them in Mojang's order.
//!
//! Doing it by path has a second payoff: a mob whose variant resolves to the
//! texture Rewo already bakes gets id **0**, the vanilla slot, and costs no
//! atlas space at all.
//!
//! # What is deliberately not here
//!
//! * **Baby textures.** Every variant ships an `_baby` sheet, and Rewo's baby
//!   is a uniform 0.5 scale of the adult model (a documented approximation
//!   since the metadata-detail pass) — so there is no baby *model* for a baby
//!   texture to sit on.
//! * **Wolf `angry`.** `Wolf.getTexture` picks between three sheets per
//!   variant, and `isAngry()` is `remainingPersistentAngerTime > 0`, i.e.
//!   `DATA_ANGER_END_TIME` (index 22, LONG) compared against the world clock —
//!   a texture that changes with *time*, not with a synched value. The wire
//!   field is not decoded and the nine angry sheets are not baked. `tame`
//!   *is*: it is a bit of a byte.
//! * **Tropical fish.** Its packed int selects a **model**
//!   (`TropicalFishSmallModel` vs `TropicalFishLargeModel`, which is what
//!   `tropical_a.png` and `tropical_b.png` belong to) plus a pattern layer and
//!   two dye tints. None of that is a texture swap on a model Rewo has, so it
//!   belongs to a mob-model milestone rather than this one.
//! * **Collars, horse markings, llama carpets.** All second render layers.

/// The variant-id band vanilla's own variants occupy.
///
/// A pack's ETF alternates use small 1-based rule indices
/// ([`crate::etf::Variant::index`]) and the emissive overlay uses a reserved id
/// above `u16` entirely, so a high band keeps all three apart in the single
/// `EntityDraw::variant` field. Nothing renders two of them at once: a pack
/// randomising `cat.png` is varying the *base* texture, and a black cat is not
/// drawing that texture at all — vanilla's variant wins where both apply.
pub const VANILLA_VARIANT_BASE: u16 = 0x4000;

/// Every alternate sheet, as `(mob-texture key, jar-relative path)`.
///
/// Position is meaning: entry `i` is variant id `VANILLA_VARIANT_BASE + i`.
/// The **base** texture of each mob is deliberately absent — it is already in
/// the atlas, and [`variant_id`] resolves it to 0.
///
/// The paths are `ClientAsset.ResourceTexture`'s expansion of the asset id the
/// registry (or the renderer's own table) names:
/// `id.withPath(p -> "textures/" + p + ".png")`, minus the `textures/` prefix
/// Rewo's mob-texture specs also drop.
const VARIANT_TEXTURES: &[(&str, &str)] = &[
    // Cat — 11 registry entries; `cat_tabby` is the baked base.
    ("cat", "entity/cat/cat_all_black.png"),
    ("cat", "entity/cat/cat_black.png"),
    ("cat", "entity/cat/cat_british_shorthair.png"),
    ("cat", "entity/cat/cat_calico.png"),
    ("cat", "entity/cat/cat_jellie.png"),
    ("cat", "entity/cat/cat_persian.png"),
    ("cat", "entity/cat/cat_ragdoll.png"),
    ("cat", "entity/cat/cat_red.png"),
    ("cat", "entity/cat/cat_siamese.png"),
    ("cat", "entity/cat/cat_white.png"),
    // Wolf — 9 registry entries x {wild, tame}; `wolf.png` (pale wild) is the
    // baked base. The nine `_angry` sheets are excluded; see the module docs.
    ("wolf", "entity/wolf/wolf_tame.png"),
    ("wolf", "entity/wolf/wolf_ashen.png"),
    ("wolf", "entity/wolf/wolf_ashen_tame.png"),
    ("wolf", "entity/wolf/wolf_black.png"),
    ("wolf", "entity/wolf/wolf_black_tame.png"),
    ("wolf", "entity/wolf/wolf_chestnut.png"),
    ("wolf", "entity/wolf/wolf_chestnut_tame.png"),
    ("wolf", "entity/wolf/wolf_rusty.png"),
    ("wolf", "entity/wolf/wolf_rusty_tame.png"),
    ("wolf", "entity/wolf/wolf_snowy.png"),
    ("wolf", "entity/wolf/wolf_snowy_tame.png"),
    ("wolf", "entity/wolf/wolf_spotted.png"),
    ("wolf", "entity/wolf/wolf_spotted_tame.png"),
    ("wolf", "entity/wolf/wolf_striped.png"),
    ("wolf", "entity/wolf/wolf_striped_tame.png"),
    ("wolf", "entity/wolf/wolf_woods.png"),
    ("wolf", "entity/wolf/wolf_woods_tame.png"),
    // Frog — 3 registry entries; `frog_temperate` is the baked base.
    ("frog", "entity/frog/frog_cold.png"),
    ("frog", "entity/frog/frog_warm.png"),
    // Axolotl — `Axolotl.Variant` 0..4; `lucy` (0) is the baked base.
    ("axolotl", "entity/axolotl/axolotl_wild.png"),
    ("axolotl", "entity/axolotl/axolotl_gold.png"),
    ("axolotl", "entity/axolotl/axolotl_cyan.png"),
    ("axolotl", "entity/axolotl/axolotl_blue.png"),
    // Llama — `Llama.Variant` 0..3; `creamy` (0) is the baked base.
    ("llama", "entity/llama/llama_white.png"),
    ("llama", "entity/llama/llama_brown.png"),
    ("llama", "entity/llama/llama_gray.png"),
    // Horse — `equine::Variant` 0..6; `brown` (3) is the baked base.
    ("horse", "entity/horse/horse_white.png"),
    ("horse", "entity/horse/horse_creamy.png"),
    ("horse", "entity/horse/horse_chestnut.png"),
    ("horse", "entity/horse/horse_black.png"),
    ("horse", "entity/horse/horse_gray.png"),
    ("horse", "entity/horse/horse_darkbrown.png"),
];

/// `AxolotlRenderer.TEXTURE_BY_TYPE`, by `Axolotl.Variant` id.
///
/// `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)` — an id outside the
/// table is LUCY, not a clamp and not a wrap.
const AXOLOTL_BY_ID: &[&str] = &[
    "entity/axolotl/axolotl_lucy.png",
    "entity/axolotl/axolotl_wild.png",
    "entity/axolotl/axolotl_gold.png",
    "entity/axolotl/axolotl_cyan.png",
    "entity/axolotl/axolotl_blue.png",
];

/// `LlamaRenderer.TEXTURES`, by `Llama.Variant` id.
/// `OutOfBoundsStrategy.CLAMP`.
const LLAMA_BY_ID: &[&str] = &[
    "entity/llama/llama_creamy.png",
    "entity/llama/llama_white.png",
    "entity/llama/llama_brown.png",
    "entity/llama/llama_gray.png",
];

/// `HorseRenderer.LOCATION_BY_VARIANT`, by `equine::Variant` id.
/// `OutOfBoundsStrategy.WRAP`.
const HORSE_BY_ID: &[&str] = &[
    "entity/horse/horse_white.png",
    "entity/horse/horse_creamy.png",
    "entity/horse/horse_chestnut.png",
    "entity/horse/horse_brown.png",
    "entity/horse/horse_black.png",
    "entity/horse/horse_gray.png",
    "entity/horse/horse_darkbrown.png",
];

/// The variant id for a jar-relative texture path.
///
/// `Some(0)` when it is the mob's own baked base texture — the vanilla slot,
/// which costs no atlas space. `None` when it is neither, which is what a
/// datapack naming a texture the jar does not ship produces; the caller then
/// leaves the mob on its base texture rather than inventing one.
pub fn variant_id(rel_path: &str) -> Option<u16> {
    if crate::assets::mob_texture_key(rel_path).is_some() {
        return Some(0);
    }
    VARIANT_TEXTURES
        .iter()
        .position(|(_, p)| *p == rel_path)
        .map(|i| VANILLA_VARIANT_BASE + i as u16)
}

/// `Axolotl.Variant.byId` → its texture, out of bounds → LUCY.
pub fn axolotl_texture(id: i32) -> &'static str {
    AXOLOTL_BY_ID
        .get(usize::try_from(id).unwrap_or(usize::MAX))
        .copied()
        .unwrap_or(AXOLOTL_BY_ID[0])
}

/// `Llama.Variant.byId` → its texture. CLAMP, so a negative id is CREAMY and
/// anything past the end is GRAY.
pub fn llama_texture(id: i32) -> &'static str {
    let i = id.clamp(0, LLAMA_BY_ID.len() as i32 - 1) as usize;
    LLAMA_BY_ID[i]
}

/// `Horse.getVariant()` → its texture.
///
/// Two vanilla details in one line: the variant is the **low byte** of the
/// synched int (`typeVariant & 0xFF`; the high byte is the markings layer,
/// which Rewo does not draw), and `equine::Variant`'s `ByIdMap` is
/// `OutOfBoundsStrategy.WRAP`, so 7 is WHITE again rather than being clamped
/// to DARK_BROWN.
pub fn horse_texture(type_variant: i32) -> &'static str {
    let n = HORSE_BY_ID.len() as i32;
    HORSE_BY_ID[((type_variant & 0xFF).rem_euclid(n)) as usize]
}

/// Every alternate as `(key, variant id, jar-relative path)`, for the bake.
pub fn specs() -> impl Iterator<Item = (&'static str, u16, &'static str)> {
    VARIANT_TEXTURES
        .iter()
        .enumerate()
        .map(|(i, (k, p))| (*k, VANILLA_VARIANT_BASE + i as u16, *p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every alternate resolves to a distinct id, and every base to 0.
    #[test]
    fn the_table_is_a_bijection_and_the_bases_are_zero() {
        let mut seen = std::collections::HashSet::new();
        for (_, id, path) in specs() {
            assert!(seen.insert(id), "duplicate variant id {id}");
            assert_eq!(variant_id(path), Some(id), "{path}");
        }
        for base in [
            "entity/cat/cat_tabby.png",
            "entity/wolf/wolf.png",
            "entity/frog/frog_temperate.png",
            "entity/axolotl/axolotl_lucy.png",
            "entity/llama/llama_creamy.png",
            "entity/horse/horse_brown.png",
        ] {
            assert_eq!(variant_id(base), Some(0), "{base} is the baked base");
        }
        assert_eq!(variant_id("entity/cat/nope.png"), None);
    }

    /// The three out-of-bounds strategies differ, and each is vanilla's.
    #[test]
    fn out_of_range_ids_follow_each_enums_own_strategy() {
        // Axolotl: ZERO.
        assert_eq!(axolotl_texture(4), "entity/axolotl/axolotl_blue.png");
        assert_eq!(axolotl_texture(5), AXOLOTL_BY_ID[0]);
        assert_eq!(axolotl_texture(-1), AXOLOTL_BY_ID[0]);
        // Llama: CLAMP.
        assert_eq!(llama_texture(3), "entity/llama/llama_gray.png");
        assert_eq!(llama_texture(9), "entity/llama/llama_gray.png");
        assert_eq!(llama_texture(-3), "entity/llama/llama_creamy.png");
        // Horse: WRAP, over the low byte only.
        assert_eq!(horse_texture(0), "entity/horse/horse_white.png");
        assert_eq!(horse_texture(6), "entity/horse/horse_darkbrown.png");
        assert_eq!(horse_texture(7), "entity/horse/horse_white.png");
        // Markings live in the high byte and must not shift the coat.
        assert_eq!(horse_texture(3 | (2 << 8)), "entity/horse/horse_brown.png");
    }
}
