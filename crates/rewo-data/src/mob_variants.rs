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
//! * **Collars, horse markings, llama carpets.** All second render layers.
//!
//! # The tropical fish (M68)
//!
//! M64 excluded it because its packed int is not a texture swap — it selects a
//! **model**, a **pattern layer** and **two dye tints**. M68 builds the model
//! and the layer, at which point the pattern *is* a texture swap on a slot
//! Rewo has, and it lands here: [`fish_pattern_variant`] maps the pattern's
//! index-within-its-shape onto this same band, so the pattern layer addresses
//! all six sheets through the one per-draw variant id every other alternate
//! uses. The two tints and the model choice stay out — a colour is not a
//! texture, and the shape is a different mesh (`rewo_gpu::mobs`).

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
    // Tropical-fish patterns (M68). Six sheets per body plan, of which
    // `_pattern_1` is the baked base of that plan's pattern slot — so five
    // alternates each, in `TropicalFish.Pattern` declaration order within the
    // shape (KOB..SPOTTY are SMALL 0..5, FLOPPER..CLAYFISH are LARGE 0..5).
    ("tropical_fish_pattern_a", "entity/fish/tropical_a_pattern_2.png"),
    ("tropical_fish_pattern_a", "entity/fish/tropical_a_pattern_3.png"),
    ("tropical_fish_pattern_a", "entity/fish/tropical_a_pattern_4.png"),
    ("tropical_fish_pattern_a", "entity/fish/tropical_a_pattern_5.png"),
    ("tropical_fish_pattern_a", "entity/fish/tropical_a_pattern_6.png"),
    ("tropical_fish_pattern_b", "entity/fish/tropical_b_pattern_2.png"),
    ("tropical_fish_pattern_b", "entity/fish/tropical_b_pattern_3.png"),
    ("tropical_fish_pattern_b", "entity/fish/tropical_b_pattern_4.png"),
    ("tropical_fish_pattern_b", "entity/fish/tropical_b_pattern_5.png"),
    ("tropical_fish_pattern_b", "entity/fish/tropical_b_pattern_6.png"),
];

// ---------------------------------------------------------------------------
// Tropical fish (M68)
// ---------------------------------------------------------------------------

/// `TropicalFish.Base` — which of the two meshes (and therefore which pair of
/// sheets) a pattern belongs to. `SMALL(0)` / `LARGE(1)`, and the id is the
/// **low bit** of the packed variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FishBase {
    Small,
    Large,
}

/// The packed `DATA_ID_TYPE_VARIANT` int, unpacked.
///
/// `TropicalFish.packVariant` is
/// `pattern.getPackedId() & 65535 | (baseColor.getId() & 0xFF) << 16 |
/// (patternColor.getId() & 0xFF) << 24`, and `Pattern`'s own packed id is
/// `base.id | index << 8`. So the four fields are, low to high:
///
/// ```text
///   bit  0      : Base   — 0 SMALL (tropical_a), 1 LARGE (tropical_b)
///   bits 8..15  : the pattern's index 0..5 *within* its Base
///   bits 16..23 : DyeColor id of the body    (`getModelTint`)
///   bits 24..31 : DyeColor id of the pattern (`TropicalFishPatternLayer`)
/// ```
///
/// Bits 1..7 belong to no field at all — they are inside `Pattern`'s 16-bit
/// half but outside both `base.id` (1 bit) and `index << 8`, which is exactly
/// why `Pattern.byId` is `ByIdMap.**sparse**(...)`: the id space is not dense,
/// so an unrecognised packed id falls back to a **named default, KOB**, rather
/// than being clamped or wrapped into a neighbour. [`FishVariant::unpack`]
/// reproduces that, which is the difference between a bogus value rendering as
/// a small orange-and-white fish (vanilla) and as garbage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FishVariant {
    pub base: FishBase,
    /// 0..5 — `TropicalFish.Pattern`'s index within its `Base`.
    pub pattern: u8,
    /// `DyeColor` id 0..15.
    pub body_color: u8,
    pub pattern_color: u8,
}

impl FishVariant {
    /// `TropicalFish.DEFAULT_VARIANT` — `(KOB, WHITE, WHITE)`, which is also
    /// what `defineSynchedData` registers, so it is what a fish renders as
    /// before any metadata arrives.
    pub const DEFAULT: Self = Self {
        base: FishBase::Small,
        pattern: 0,
        body_color: 0,
        pattern_color: 0,
    };

    /// Unpack, with `Pattern.byId`'s sparse fallback.
    pub fn unpack(packed: i32) -> Self {
        let pattern_id = packed & 0xFFFF;
        // `ByIdMap.sparse` over the twelve declared `packedId`s; anything else
        // is KOB. A declared id is exactly `base | index << 8` with base in
        // 0..=1 and index in 0..=5, so *reconstructing* it and comparing is
        // the membership test — and it rejects the bits 1..7 that belong to no
        // field, which a mask over "the bits the fields use" would let through.
        let base_bit = pattern_id & 1;
        let index = pattern_id >> 8;
        let (base_bit, index) = if index <= 5 && pattern_id == (base_bit | (index << 8)) {
            (base_bit, index)
        } else {
            (0, 0) // KOB
        };
        Self {
            base: if base_bit == 1 { FishBase::Large } else { FishBase::Small },
            pattern: index as u8,
            // `DyeColor.byId` is `ByIdMap.continuous(..., WRAP)` over 16.
            body_color: ((packed >> 16) & 0xFF).rem_euclid(16) as u8,
            pattern_color: ((packed >> 24) & 0xFF).rem_euclid(16) as u8,
        }
    }
}

/// The mob-texture key of a body plan's **pattern** slot.
pub fn fish_pattern_key(base: FishBase) -> &'static str {
    match base {
        FishBase::Small => "tropical_fish_pattern_a",
        FishBase::Large => "tropical_fish_pattern_b",
    }
}

/// Every pattern sheet, `[SMALL, LARGE][index 0..5]` — `_pattern_1..6`, in
/// `TropicalFish.Pattern` declaration order within each `Base`.
const FISH_PATTERN_PATHS: [[&str; 6]; 2] = [
    [
        "entity/fish/tropical_a_pattern_1.png",
        "entity/fish/tropical_a_pattern_2.png",
        "entity/fish/tropical_a_pattern_3.png",
        "entity/fish/tropical_a_pattern_4.png",
        "entity/fish/tropical_a_pattern_5.png",
        "entity/fish/tropical_a_pattern_6.png",
    ],
    [
        "entity/fish/tropical_b_pattern_1.png",
        "entity/fish/tropical_b_pattern_2.png",
        "entity/fish/tropical_b_pattern_3.png",
        "entity/fish/tropical_b_pattern_4.png",
        "entity/fish/tropical_b_pattern_5.png",
        "entity/fish/tropical_b_pattern_6.png",
    ],
];

/// The variant id that moves a body plan's pattern slot onto `pattern`'s
/// sheet.
///
/// Resolved through [`variant_id`], i.e. through the **texture path**, which
/// is this module's one join and keeps index 0 answering `0` (the baked base
/// of that slot) without a second table saying so. An index past the end is
/// unreachable — [`FishVariant::unpack`] has already applied vanilla's sparse
/// fallback — and answers `0` rather than panicking.
pub fn fish_pattern_variant(base: FishBase, pattern: u8) -> u16 {
    let row = match base {
        FishBase::Small => 0,
        FishBase::Large => 1,
    };
    FISH_PATTERN_PATHS[row]
        .get(pattern as usize)
        .and_then(|p| variant_id(p))
        .unwrap_or(0)
}

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

    /// `TropicalFish.packVariant`, rebuilt here from the decompiled formula
    /// rather than by calling the unpacker's inverse, so the two can disagree.
    fn pack(base: i32, index: i32, body: i32, pattern: i32) -> i32 {
        ((base | (index << 8)) & 0xFFFF) | ((body & 0xFF) << 16) | ((pattern & 0xFF) << 24)
    }

    /// Each of the four fields comes out of its own bits, and moving one moves
    /// nothing else — the property that a wrong shift would break.
    #[test]
    fn the_packed_fish_variant_splits_into_its_four_independent_fields() {
        // `COMMON_VARIANTS[7]`: BLOCKFISH (LARGE, index 3), purple, yellow.
        let v = FishVariant::unpack(pack(1, 3, 10, 4));
        assert_eq!(v.base, FishBase::Large);
        assert_eq!(v.pattern, 3);
        assert_eq!(v.body_color, 10);
        assert_eq!(v.pattern_color, 4);
        // The shape is the LOW bit, not a byte: SUNSTREAK is SMALL index 1,
        // and STRIPEY is LARGE index 1 — same index, different mesh.
        assert_eq!(FishVariant::unpack(pack(0, 1, 0, 0)).base, FishBase::Small);
        assert_eq!(FishVariant::unpack(pack(1, 1, 0, 0)).base, FishBase::Large);
        // Independence: each field moved on its own.
        let a = FishVariant::unpack(pack(0, 0, 0, 0));
        assert_eq!(a, FishVariant::DEFAULT, "the default variant is (KOB, WHITE, WHITE)");
        assert_eq!(FishVariant::unpack(pack(0, 0, 15, 0)).pattern_color, 0);
        assert_eq!(FishVariant::unpack(pack(0, 0, 0, 15)).body_color, 0);
        assert_eq!(FishVariant::unpack(pack(0, 5, 0, 0)).pattern, 5);
        let v = FishVariant::unpack(pack(1, 5, 15, 15));
        assert_eq!((v.body_color, v.pattern_color), (15, 15));
        assert_eq!((v.base, v.pattern), (FishBase::Large, 5));
        // No real variant sets bit 31 — `(id & 0xFF) << 24` needs id >= 128
        // and `DyeColor` stops at 15 — but the wire carries a plain `int` and
        // a bogus one must not turn the shift into an arithmetic-shift smear.
        assert!(pack(1, 5, 15, 15) > 0, "a real packed variant is non-negative");
        let v = FishVariant::unpack(-1);
        assert_eq!((v.body_color, v.pattern_color), (15, 15), "0xFF wraps to BLACK, not out of range");
    }

    /// `Pattern.byId` is `ByIdMap.**sparse**`, so an id that is not one of the
    /// twelve declared `packedId`s is KOB — not a clamp, not a wrap, and not
    /// "whatever the bits say".
    #[test]
    fn an_undeclared_pattern_id_falls_back_to_kob() {
        // Index 6 does not exist for either shape.
        assert_eq!(FishVariant::unpack(pack(1, 6, 3, 7)).pattern, 0);
        assert_eq!(FishVariant::unpack(pack(1, 6, 3, 7)).base, FishBase::Small);
        // …and the colours are unaffected: only `Pattern` goes through `byId`.
        let v = FishVariant::unpack(pack(1, 9, 3, 7));
        assert_eq!((v.body_color, v.pattern_color), (3, 7));
        // Bits 1..7 belong to no field. A mask-based membership test would
        // accept them and read a valid pattern; vanilla's does not.
        assert_eq!(FishVariant::unpack(0x0102).pattern, 0, "bit 1 is not part of Base");
        assert_eq!(FishVariant::unpack(0x0102).base, FishBase::Small);
        // The twelve declared ids all survive.
        for base in 0..2 {
            for index in 0..6 {
                let v = FishVariant::unpack(pack(base, index, 0, 0));
                assert_eq!(v.pattern, index as u8, "base {base} index {index}");
                assert_eq!(v.base == FishBase::Large, base == 1);
            }
        }
    }

    /// Pattern 0 is the slot's baked base (id 0, no atlas cost); the other
    /// five resolve to distinct ids, and the two shapes never share one.
    #[test]
    fn every_fish_pattern_resolves_to_its_own_slot() {
        let mut seen = std::collections::HashSet::new();
        for base in [FishBase::Small, FishBase::Large] {
            assert_eq!(fish_pattern_variant(base, 0), 0, "index 0 is the baked base");
            for i in 1..6u8 {
                let id = fish_pattern_variant(base, i);
                assert_ne!(id, 0, "{base:?} pattern {i} did not resolve");
                assert!(seen.insert(id), "{base:?} pattern {i} duplicates id {id}");
            }
        }
        assert_eq!(fish_pattern_key(FishBase::Small), "tropical_fish_pattern_a");
        assert_eq!(fish_pattern_key(FishBase::Large), "tropical_fish_pattern_b");
    }
}
