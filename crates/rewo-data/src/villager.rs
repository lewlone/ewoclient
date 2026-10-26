//! `VillagerProfessionLayer` data: which overlay sheets a villager or zombie
//! villager wears, and the registry names that choose them.
//!
//! `VillagerData.STREAM_CODEC` is `holderRegistry(VILLAGER_TYPE)`,
//! `holderRegistry(VILLAGER_PROFESSION)` and a `VAR_INT` level: the first two
//! are RAW 0-based ids of **built-in** registries, so the report is the
//! authority for them ([`VillagerRegistries`]), never the wire and never a
//! hardcoded order.
//!
//! The layer (`VillagerProfessionLayer.submit`) draws, on the parent model:
//!
//! 1. the biome type sheet (`textures/entity/<family>/type/<type>.png`, or
//!    `baby/<type>.png` for a baby), on the full model when `typeHatVisible`
//!    and on the no-hat model (the head subtree cleared) when not;
//! 2. unless baby, and unless the profession is `none`: the profession sheet
//!    (`profession/<profession>.png`);
//! 3. and unless the profession is `nitwit` too: the level badge
//!    (`profession_level/<stone|iron|gold|emerald|diamond>.png`, level
//!    clamped to `1..=5`).
//!
//! `typeHatVisible` is read from the `.png.mcmeta` `"villager": {"hat": ..}`
//! sections of the TYPE and PROFESSION sheets (`VillagerMetadataSection`),
//! each defaulting to `none` when the sheet has no such section. They are
//! tabulated below ([`hat`]) and a jar-reading test grades the table.

use std::collections::HashMap;
use std::path::Path;

use crate::read_json_file;

/// `VillagerMetadataSection.Hat`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hat {
    None,
    Partial,
    Full,
}

/// Which folder of a family a sheet lives in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Folder {
    /// `baby/<type>.png`
    Baby,
    /// `type/<type>.png`
    Kind,
    /// `profession/<profession>.png`
    Profession,
    /// `profession_level/<level>.png`
    Level,
}

/// One overlay sheet. `key` is its name in the entity atlas.
#[derive(Clone, Copy, Debug)]
pub struct Sheet {
    pub key: &'static str,
    /// Jar-relative path under `assets/minecraft/textures/`.
    pub path: &'static str,
    pub zombie: bool,
    pub folder: Folder,
    pub name: &'static str,
}

/// Every sheet is 64 x 64, like the base villager sheets (the layer re-uses
/// the base model's UVs).
pub const SHEET_SIZE: u32 = 64;

macro_rules! sheet {
    (villager, baby, $n:literal) => {
        sheet!(@ "villager", false, Baby, "baby", $n)
    };
    (villager, kind, $n:literal) => {
        sheet!(@ "villager", false, Kind, "type", $n)
    };
    (villager, profession, $n:literal) => {
        sheet!(@ "villager", false, Profession, "profession", $n)
    };
    (villager, level, $n:literal) => {
        sheet!(@ "villager", false, Level, "profession_level", $n)
    };
    (zombie_villager, baby, $n:literal) => {
        sheet!(@ "zombie_villager", true, Baby, "baby", $n)
    };
    (zombie_villager, kind, $n:literal) => {
        sheet!(@ "zombie_villager", true, Kind, "type", $n)
    };
    (zombie_villager, profession, $n:literal) => {
        sheet!(@ "zombie_villager", true, Profession, "profession", $n)
    };
    (zombie_villager, level, $n:literal) => {
        sheet!(@ "zombie_villager", true, Level, "profession_level", $n)
    };
    (@ $fam:literal, $zombie:literal, $folder:ident, $dir:literal, $n:literal) => {
        Sheet {
            key: concat!($fam, "/", $dir, "/", $n),
            path: concat!("entity/", $fam, "/", $dir, "/", $n, ".png"),
            zombie: $zombie,
            folder: Folder::$folder,
            name: $n,
        }
    };
}

/// Every overlay sheet, both families. Order is not meaning; look sheets up
/// with [`sheet_key`].
pub const SHEETS: &[Sheet] = &[
    sheet!(villager, baby, "desert"),
    sheet!(villager, baby, "jungle"),
    sheet!(villager, baby, "plains"),
    sheet!(villager, baby, "savanna"),
    sheet!(villager, baby, "snow"),
    sheet!(villager, baby, "swamp"),
    sheet!(villager, baby, "taiga"),
    sheet!(villager, kind, "desert"),
    sheet!(villager, kind, "jungle"),
    sheet!(villager, kind, "plains"),
    sheet!(villager, kind, "savanna"),
    sheet!(villager, kind, "snow"),
    sheet!(villager, kind, "swamp"),
    sheet!(villager, kind, "taiga"),
    sheet!(villager, profession, "armorer"),
    sheet!(villager, profession, "butcher"),
    sheet!(villager, profession, "cartographer"),
    sheet!(villager, profession, "cleric"),
    sheet!(villager, profession, "farmer"),
    sheet!(villager, profession, "fisherman"),
    sheet!(villager, profession, "fletcher"),
    sheet!(villager, profession, "leatherworker"),
    sheet!(villager, profession, "librarian"),
    sheet!(villager, profession, "mason"),
    sheet!(villager, profession, "nitwit"),
    sheet!(villager, profession, "shepherd"),
    sheet!(villager, profession, "toolsmith"),
    sheet!(villager, profession, "weaponsmith"),
    sheet!(villager, level, "stone"),
    sheet!(villager, level, "iron"),
    sheet!(villager, level, "gold"),
    sheet!(villager, level, "emerald"),
    sheet!(villager, level, "diamond"),
    sheet!(zombie_villager, baby, "desert"),
    sheet!(zombie_villager, baby, "jungle"),
    sheet!(zombie_villager, baby, "plains"),
    sheet!(zombie_villager, baby, "savanna"),
    sheet!(zombie_villager, baby, "snow"),
    sheet!(zombie_villager, baby, "swamp"),
    sheet!(zombie_villager, baby, "taiga"),
    sheet!(zombie_villager, kind, "desert"),
    sheet!(zombie_villager, kind, "jungle"),
    sheet!(zombie_villager, kind, "plains"),
    sheet!(zombie_villager, kind, "savanna"),
    sheet!(zombie_villager, kind, "snow"),
    sheet!(zombie_villager, kind, "swamp"),
    sheet!(zombie_villager, kind, "taiga"),
    sheet!(zombie_villager, profession, "armorer"),
    sheet!(zombie_villager, profession, "butcher"),
    sheet!(zombie_villager, profession, "cartographer"),
    sheet!(zombie_villager, profession, "cleric"),
    sheet!(zombie_villager, profession, "farmer"),
    sheet!(zombie_villager, profession, "fisherman"),
    sheet!(zombie_villager, profession, "fletcher"),
    sheet!(zombie_villager, profession, "leatherworker"),
    sheet!(zombie_villager, profession, "librarian"),
    sheet!(zombie_villager, profession, "mason"),
    sheet!(zombie_villager, profession, "nitwit"),
    sheet!(zombie_villager, profession, "shepherd"),
    sheet!(zombie_villager, profession, "toolsmith"),
    sheet!(zombie_villager, profession, "weaponsmith"),
    sheet!(zombie_villager, level, "stone"),
    sheet!(zombie_villager, level, "iron"),
    sheet!(zombie_villager, level, "gold"),
    sheet!(zombie_villager, level, "emerald"),
    sheet!(zombie_villager, level, "diamond"),
];

/// `VillagerProfessionLayer.LEVEL_LOCATIONS`, level 1 first.
pub const LEVEL_NAMES: [&str; 5] = ["stone", "iron", "gold", "emerald", "diamond"];

/// The atlas key of the sheet in `folder` named `name` (no namespace), or
/// `None` when the jar has none.
pub fn sheet_key(zombie: bool, folder: Folder, name: &str) -> Option<&'static str> {
    SHEETS
        .iter()
        .find(|s| s.zombie == zombie && s.folder == folder && s.name == name)
        .map(|s| s.key)
}

/// The `"villager": {"hat": ..}` section of a TYPE or PROFESSION sheet's
/// `.png.mcmeta` (`VillagerProfessionLayer.getHatData`); `Hat::None` for a
/// sheet with no such section, and for a name with no sheet at all (an absent
/// resource is `Optional.empty()` → NONE). Only `Folder::Kind` and
/// `Folder::Profession` carry the section.
///
/// Read from the 26.2 jar: the villager's `desert` and `snow` types are
/// `full`, the zombie villager's types have none; the profession sections are
/// the same for both families.
pub fn hat(zombie: bool, folder: Folder, name: &str) -> Hat {
    match (folder, name) {
        (Folder::Kind, "desert" | "snow") if !zombie => Hat::Full,
        (Folder::Profession, "butcher") => Hat::Partial,
        (Folder::Profession, "farmer" | "fisherman" | "fletcher" | "librarian" | "shepherd") => {
            Hat::Full
        }
        _ => Hat::None,
    }
}

/// What `VillagerProfessionLayer.submit` draws for one villager.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LayerPlan {
    /// The biome sheet (`baby/<type>` for a baby), `None` if the jar has none.
    pub type_key: Option<&'static str>,
    /// Whether the type sheet draws on the full model (`typeHatVisible`) or
    /// on the no-hat model, which has no head subtree at all.
    pub type_with_head: bool,
    pub profession_key: Option<&'static str>,
    pub level_key: Option<&'static str>,
}

/// `VillagerProfessionLayer.submit` for one `VillagerData`, given as registry
/// names (with or without the `minecraft:` namespace; another namespace has
/// no sheet).
pub fn plan(
    zombie: bool,
    type_name: &str,
    profession_name: &str,
    level: i32,
    baby: bool,
) -> Option<LayerPlan> {
    let type_name = vanilla_path(type_name)?;
    let profession_name = vanilla_path(profession_name)?;
    let type_hat = hat(zombie, Folder::Kind, type_name);
    let profession_hat = hat(zombie, Folder::Profession, profession_name);
    let type_with_head =
        profession_hat == Hat::None || (profession_hat == Hat::Partial && type_hat != Hat::Full);
    let type_folder = if baby { Folder::Baby } else { Folder::Kind };
    let has_profession = profession_name != "none" && !baby;
    let level_name = LEVEL_NAMES[(level.clamp(1, LEVEL_NAMES.len() as i32) - 1) as usize];
    Some(LayerPlan {
        type_key: sheet_key(zombie, type_folder, type_name),
        type_with_head,
        profession_key: sheet_key(zombie, Folder::Profession, profession_name)
            .filter(|_| has_profession),
        level_key: sheet_key(zombie, Folder::Level, level_name)
            .filter(|_| has_profession && profession_name != "nitwit"),
    })
}

/// The path of a `minecraft:` registry name (or a bare name).
fn vanilla_path(name: &str) -> Option<&str> {
    match name.split_once(':') {
        None => Some(name),
        Some(("minecraft", path)) => Some(path),
        Some(_) => None,
    }
}

/// The `minecraft:villager_type` and `minecraft:villager_profession`
/// registries: protocol id to registry name.
#[derive(Clone, Default)]
pub struct VillagerRegistries {
    types: HashMap<i32, String>,
    professions: HashMap<i32, String>,
}

impl VillagerRegistries {
    pub fn load(path: &Path) -> Result<Self, String> {
        Self::from_value(&read_json_file(path)?)
    }

    pub fn from_value(json: &serde_json::Value) -> Result<Self, String> {
        Ok(Self {
            types: registry(json, "minecraft:villager_type")?,
            professions: registry(json, "minecraft:villager_profession")?,
        })
    }

    /// A table from literal pairs, for fixtures.
    pub fn from_pairs(types: &[(i32, &str)], professions: &[(i32, &str)]) -> Self {
        let own = |p: &[(i32, &str)]| p.iter().map(|(i, n)| (*i, n.to_string())).collect();
        Self {
            types: own(types),
            professions: own(professions),
        }
    }

    pub fn type_name(&self, id: i32) -> Option<&str> {
        self.types.get(&id).map(String::as_str)
    }

    pub fn profession_name(&self, id: i32) -> Option<&str> {
        self.professions.get(&id).map(String::as_str)
    }

    /// [`plan`] for the raw `(type id, profession id, level)` the wire carries.
    /// `None` for an id the report does not list (vanilla's `holderRegistry`
    /// would have failed the decode).
    pub fn plan(&self, zombie: bool, data: (i32, i32, i32), baby: bool) -> Option<LayerPlan> {
        plan(
            zombie,
            self.type_name(data.0)?,
            self.profession_name(data.1)?,
            data.2,
            baby,
        )
    }
}

fn registry(json: &serde_json::Value, name: &str) -> Result<HashMap<i32, String>, String> {
    let entries = json
        .get(name)
        .and_then(|r| r.get("entries"))
        .and_then(|e| e.as_object())
        .ok_or_else(|| format!("registries.json: no {name} registry"))?;
    let mut out = HashMap::with_capacity(entries.len());
    for (entry, body) in entries {
        // `protocol_id`, never the iteration index: the map is sorted by name.
        let id = body
            .get("protocol_id")
            .and_then(|i| i.as_i64())
            .and_then(|i| i32::try_from(i).ok())
            .ok_or_else(|| format!("registries.json: {name} {entry} has no protocol_id"))?;
        out.insert(id, entry.clone());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TYPES: [&str; 7] = [
        "desert", "jungle", "plains", "savanna", "snow", "swamp", "taiga",
    ];
    const PROFESSIONS: [&str; 14] = [
        "armorer",
        "butcher",
        "cartographer",
        "cleric",
        "farmer",
        "fisherman",
        "fletcher",
        "leatherworker",
        "librarian",
        "mason",
        "nitwit",
        "shepherd",
        "toolsmith",
        "weaponsmith",
    ];

    /// The sheet table is exactly the two families x four folders it names,
    /// every key distinct and every path the key's `entity/` expansion.
    #[test]
    fn the_sheet_table_is_the_full_cross_product() {
        let mut want = Vec::new();
        for (zombie, fam) in [(false, "villager"), (true, "zombie_villager")] {
            for (folder, dir, names) in [
                (Folder::Baby, "baby", &TYPES[..]),
                (Folder::Kind, "type", &TYPES[..]),
                (Folder::Profession, "profession", &PROFESSIONS[..]),
                (Folder::Level, "profession_level", &LEVEL_NAMES[..]),
            ] {
                for n in names {
                    want.push((zombie, folder, *n, format!("{fam}/{dir}/{n}")));
                }
            }
        }
        assert_eq!(SHEETS.len(), want.len());
        for (zombie, folder, name, key) in &want {
            let s = SHEETS
                .iter()
                .filter(|s| s.zombie == *zombie && s.folder == *folder && s.name == *name)
                .collect::<Vec<_>>();
            assert_eq!(s.len(), 1, "{key}");
            assert_eq!(s[0].key, key);
            assert_eq!(s[0].path, format!("entity/{key}.png"));
        }
    }

    fn plan_of(zombie: bool, ty: &str, prof: &str, level: i32, baby: bool) -> LayerPlan {
        plan(zombie, ty, prof, level, baby).expect("a vanilla name")
    }

    /// No profession: the type sheet on the full model and nothing else
    /// (`profession.is(NONE)` skips both further layers).
    #[test]
    fn a_jobless_villager_wears_only_its_biome() {
        let p = plan_of(false, "minecraft:plains", "minecraft:none", 1, false);
        assert_eq!(p.type_key, Some("villager/type/plains"));
        assert!(p.type_with_head);
        assert_eq!(p.profession_key, None);
        assert_eq!(p.level_key, None);
    }

    /// The playtest villager: plains / farmer / level 1. The farmer's hat is
    /// `full`, so the type sheet draws on the no-hat model; the profession
    /// and the stone badge follow.
    #[test]
    fn a_plains_farmer_wears_three_layers_and_hides_the_type_head() {
        let p = plan_of(false, "minecraft:plains", "minecraft:farmer", 1, false);
        assert_eq!(p.type_key, Some("villager/type/plains"));
        assert!(!p.type_with_head);
        assert_eq!(p.profession_key, Some("villager/profession/farmer"));
        assert_eq!(p.level_key, Some("villager/profession_level/stone"));
    }

    /// `typeHatVisible = professionHat == NONE || (professionHat == PARTIAL
    /// && typeHat != FULL)`: a butcher (partial) keeps the head over a
    /// plains type and loses it over a desert one (the desert type is full).
    #[test]
    fn a_partial_profession_hat_yields_to_a_full_type_hat() {
        assert!(plan_of(false, "plains", "butcher", 2, false).type_with_head);
        assert!(!plan_of(false, "desert", "butcher", 2, false).type_with_head);
        // The zombie villager's desert sheet has no mcmeta, so no yielding.
        assert!(plan_of(true, "desert", "butcher", 2, false).type_with_head);
        // A profession with no mcmeta never hides the head.
        assert!(plan_of(false, "snow", "cleric", 2, false).type_with_head);
    }

    /// A nitwit has a profession sheet and no badge; the level clamps to
    /// `1..=5` (`Mth.clamp(level, 1, 5)`).
    #[test]
    fn a_nitwit_has_no_badge_and_the_level_clamps() {
        let p = plan_of(false, "plains", "nitwit", 3, false);
        assert_eq!(p.profession_key, Some("villager/profession/nitwit"));
        assert_eq!(p.level_key, None);
        for (level, badge) in [
            (-4, "stone"),
            (0, "stone"),
            (1, "stone"),
            (2, "iron"),
            (3, "gold"),
            (4, "emerald"),
            (5, "diamond"),
            (6, "diamond"),
            (i32::MAX, "diamond"),
        ] {
            let p = plan_of(false, "plains", "librarian", level, false);
            assert_eq!(
                p.level_key,
                Some(match badge {
                    "stone" => "villager/profession_level/stone",
                    "iron" => "villager/profession_level/iron",
                    "gold" => "villager/profession_level/gold",
                    "emerald" => "villager/profession_level/emerald",
                    _ => "villager/profession_level/diamond",
                }),
                "level {level}"
            );
        }
    }

    /// A baby wears the `baby/<type>` sheet and never a profession or badge,
    /// but `typeHatVisible` still comes from the adult sheets' mcmeta.
    #[test]
    fn a_baby_wears_only_its_baby_biome_sheet() {
        let p = plan_of(false, "minecraft:swamp", "minecraft:farmer", 4, true);
        assert_eq!(p.type_key, Some("villager/baby/swamp"));
        assert!(!p.type_with_head, "the farmer's full hat still applies");
        assert_eq!(p.profession_key, None);
        assert_eq!(p.level_key, None);
        let z = plan_of(true, "taiga", "none", 1, true);
        assert_eq!(z.type_key, Some("zombie_villager/baby/taiga"));
    }

    /// The zombie villager reads its own folder.
    #[test]
    fn the_zombie_villager_uses_its_own_sheets() {
        let p = plan_of(true, "minecraft:savanna", "minecraft:mason", 5, false);
        assert_eq!(p.type_key, Some("zombie_villager/type/savanna"));
        assert_eq!(p.profession_key, Some("zombie_villager/profession/mason"));
        assert_eq!(
            p.level_key,
            Some("zombie_villager/profession_level/diamond")
        );
    }

    /// A name with no sheet draws nothing for that layer (vanilla would draw
    /// the missing texture), and another namespace has no sheet at all.
    #[test]
    fn unknown_names_have_no_sheet() {
        let p = plan_of(false, "minecraft:moon", "minecraft:wizard", 1, false);
        assert_eq!(p.type_key, None);
        assert_eq!(p.profession_key, None);
        assert_eq!(p.level_key, Some("villager/profession_level/stone"));
        assert_eq!(plan(false, "mod:plains", "minecraft:none", 1, false), None);
    }

    // ---- against the real jar and report ----------------------------------

    fn jar() -> Option<std::path::PathBuf> {
        let paths = crate::DataPaths::for_version("26.2")?;
        let jar = paths
            .root
            .parent()?
            .parent()?
            .join("shared/versions/26.2/26.2.jar");
        jar.is_file().then_some(jar)
    }

    /// Every table sheet exists in the jar at 64 x 64, and the hat table is
    /// the jar's own `.png.mcmeta` sections (absent section: `none`).
    #[test]
    fn the_sheets_and_hats_match_the_jar() {
        let Some(jar) = jar() else {
            crate::skip_test!("no local 26.2 client jar");
            return;
        };
        let mut z = zip::ZipArchive::new(std::fs::File::open(jar).unwrap()).unwrap();
        for s in SHEETS {
            let path = format!("assets/minecraft/textures/{}", s.path);
            let mut png = Vec::new();
            std::io::Read::read_to_end(&mut z.by_name(&path).expect(&path), &mut png).unwrap();
            let (_, w, h) = crate::assets::decode_entity_png(&png).expect(&path);
            assert_eq!((w, h), (SHEET_SIZE, SHEET_SIZE), "{path}");
            if matches!(s.folder, Folder::Kind | Folder::Profession) {
                let meta = z
                    .by_name(&format!("{path}.mcmeta"))
                    .ok()
                    .map(|mut f| {
                        let mut t = String::new();
                        std::io::Read::read_to_string(&mut f, &mut t).unwrap();
                        serde_json::from_str::<serde_json::Value>(&t).unwrap()
                    })
                    .and_then(|v| {
                        v.get("villager")
                            .and_then(|v| v.get("hat"))
                            .and_then(|h| h.as_str().map(str::to_owned))
                    });
                let want = match meta.as_deref() {
                    None | Some("none") => Hat::None,
                    Some("partial") => Hat::Partial,
                    Some("full") => Hat::Full,
                    Some(other) => panic!("{path}: hat {other}"),
                };
                assert_eq!(hat(s.zombie, s.folder, s.name), want, "{path}");
            }
        }
    }

    /// The report lists exactly the types and professions the table knows
    /// (professions plus `none`), and the ids it gives resolve through
    /// [`VillagerRegistries::plan`].
    #[test]
    fn the_report_names_what_the_table_knows() {
        let Some(paths) = crate::DataPaths::for_version("26.2") else {
            crate::skip_test!("no config dir");
            return;
        };
        let report = paths.registries_json();
        if !report.exists() {
            crate::skip_test!("no datagen report at {}", report.display());
            return;
        }
        let json = read_json_file(&report).unwrap();
        let regs = VillagerRegistries::from_value(&json).unwrap();
        let entries = |name: &str| {
            let mut v: Vec<(i64, String)> = json[name]["entries"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(n, b)| (b["protocol_id"].as_i64().unwrap(), n.clone()))
                .collect();
            v.sort();
            v
        };
        let types = entries("minecraft:villager_type");
        assert_eq!(
            types.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>(),
            TYPES.map(|t| format!("minecraft:{t}")).to_vec()
        );
        let professions = entries("minecraft:villager_profession");
        assert_eq!(professions[0].1, "minecraft:none");
        assert_eq!(
            professions[1..]
                .iter()
                .map(|(_, n)| n.as_str())
                .collect::<Vec<_>>(),
            PROFESSIONS.map(|t| format!("minecraft:{t}")).to_vec()
        );
        for (id, name) in &types {
            assert_eq!(regs.type_name(*id as i32), Some(name.as_str()));
        }
        for (id, name) in &professions {
            assert_eq!(regs.profession_name(*id as i32), Some(name.as_str()));
        }
        // The playtest villager through the ids: plains, farmer, level 1.
        let id_of = |list: &[(i64, String)], n: &str| {
            list.iter().find(|(_, name)| name == n).unwrap().0 as i32
        };
        let plains = id_of(&types, "minecraft:plains");
        let farmer = id_of(&professions, "minecraft:farmer");
        let p = regs.plan(false, (plains, farmer, 1), false).unwrap();
        assert_eq!(p.profession_key, Some("villager/profession/farmer"));
        assert_eq!(regs.plan(false, (99, farmer, 1), false), None);
    }
}
