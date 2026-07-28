//! ETF — OptiFine **Random Entities** textures (REWO_PLAN §12 M9b), the
//! texture half of the EMF/ETF-equivalent that [`crate::cem`] started.
//!
//! A pack ships a `.properties` file per vanilla entity texture listing
//! alternates, each with an optional weight and conditions; a client picks
//! one per entity, stably, so a herd of cows is not all the same cow.
//!
//! ## Provenance — read this before trusting the details
//!
//! Unlike every other Rewo subsystem, this one has **no decompile ground
//! truth**: random entity textures are an OptiFine feature, not a vanilla
//! one, and OptiFine is closed source. What is transcribed here is its
//! *documented* `random_entities.properties` format. Two consequences are
//! stated plainly rather than papered over:
//!
//! 1. **The choice function is ours, not OptiFine's.** OptiFine's exact
//!    hash is unpublished. [`EtfPack::pick`] uses a documented splitmix
//!    over the entity UUID, which reproduces the properties that matter —
//!    stable per entity, uniform, weight-respecting — but will not hand the
//!    *same* cow the same variant OptiFine would. Nothing syncs this
//!    between clients in vanilla either, so it is cosmetic.
//! 2. **Conditions we cannot evaluate never match.** Rewo does not decode
//!    biomes, health, villager professions, collar colours, weather, or
//!    entity NBT, so a rule carrying one of those is skipped entirely
//!    ([`Unsupported`]). That direction is deliberate: skipping falls back
//!    to the vanilla texture, whereas assuming a match would paint swamp
//!    textures on every mob everywhere.
//!
//! Supported conditions are the ones Rewo genuinely knows:
//! `weights`, `names`, `baby`, `sizes`, `heights`, `moonPhase`, `dayTime`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Where a pack may put its random-entity properties, each mirroring the
/// layout under `assets/minecraft/textures/`. The second is the MCPatcher
/// era's spelling, which OptiFine still reads.
const PROPS_DIRS: [&str; 2] = [
    "assets/minecraft/optifine/random/entity/",
    "assets/minecraft/mcpatcher/mob/",
];
const TEXTURES_ROOT: &str = "assets/minecraft/textures/";

/// A condition Rewo has no data for. The rule carrying it never matches;
/// the name is kept so the load log can say which one, once.
pub type Unsupported = &'static str;

/// An inclusive integer range from the properties syntax: `3`, `3-7`,
/// `-7` (up to 7) or `3-` (from 3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IntRange {
    pub lo: i64,
    pub hi: i64,
}

impl IntRange {
    fn contains(&self, v: i64) -> bool {
        v >= self.lo && v <= self.hi
    }

    /// Parse one range token. `None` for anything unparseable — the caller
    /// treats a rule with a broken list as unsupported rather than guessing.
    fn parse(tok: &str) -> Option<IntRange> {
        let t = tok.trim();
        if t.is_empty() {
            return None;
        }
        // A bare negative number ("-3") is a single value, not an open
        // range.
        if let Ok(v) = t.parse::<i64>() {
            return Some(IntRange { lo: v, hi: v });
        }
        // Otherwise one of the dashes is the separator and the others are
        // signs ("-64--30" is y −64..−30). Only the split that leaves two
        // readable halves can be the right one, so try each in turn.
        let bound = |s: &str, open: i64| -> Option<i64> {
            let s = s.trim();
            if s.is_empty() {
                Some(open)
            } else {
                s.parse().ok()
            }
        };
        t.char_indices()
            .filter(|(i, c)| *c == '-' && *i > 0)
            .find_map(|(i, _)| {
                let lo = bound(&t[..i], i64::MIN)?;
                let hi = bound(&t[i + 1..], i64::MAX)?;
                (lo <= hi).then_some(IntRange { lo, hi })
            })
    }
}

/// A `names.N` entry. OptiFine allows plain text plus `pattern:`/`regex:`
/// forms and their case-insensitive `i` variants; we implement the plain
/// and wildcard forms and treat regex as unsupported (a rule using it is
/// skipped, per the module note).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameMatch {
    Exact(String),
    /// `pattern:` — `*` matches any run, `?` any single character.
    Pattern { glob: String, ignore_case: bool },
}

impl NameMatch {
    fn matches(&self, name: &str) -> bool {
        match self {
            NameMatch::Exact(s) => s == name,
            NameMatch::Pattern { glob, ignore_case } => {
                if *ignore_case {
                    glob_match(&glob.to_lowercase(), &name.to_lowercase())
                } else {
                    glob_match(glob, name)
                }
            }
        }
    }
}

/// `*`/`?` wildcard match, iterative with backtracking (no recursion, so a
/// pathological pattern can't blow the stack).
fn glob_match(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// The conditions on one variant. All present ones must hold (OptiFine
/// ANDs across condition kinds and ORs within a list).
#[derive(Clone, Default, Debug)]
pub struct Conditions {
    pub names: Vec<NameMatch>,
    pub baby: Option<bool>,
    pub sizes: Vec<IntRange>,
    pub heights: Vec<IntRange>,
    pub moon_phases: Vec<IntRange>,
    pub day_times: Vec<IntRange>,
    /// Set when the rule carries a condition we cannot evaluate — it then
    /// never matches. See the module docs.
    pub unsupported: Option<Unsupported>,
}

impl Conditions {
    fn matches(&self, e: &EntityProps<'_>) -> bool {
        if self.unsupported.is_some() {
            return false;
        }
        if !self.names.is_empty() {
            let Some(name) = e.name else { return false };
            if !self.names.iter().any(|n| n.matches(name)) {
                return false;
            }
        }
        if self.baby.is_some_and(|b| b != e.baby) {
            return false;
        }
        // A size condition on an entity with no size (anything but a slime
        // or magma cube) cannot hold.
        if !self.sizes.is_empty() {
            let Some(size) = e.size else { return false };
            if !self.sizes.iter().any(|r| r.contains(size as i64)) {
                return false;
            }
        }
        if !self.heights.is_empty() && !self.heights.iter().any(|r| r.contains(e.y as i64)) {
            return false;
        }
        if !self.moon_phases.is_empty() {
            let phase = (e.day_ticks.div_euclid(24_000)).rem_euclid(8);
            if !self.moon_phases.iter().any(|r| r.contains(phase)) {
                return false;
            }
        }
        if !self.day_times.is_empty() {
            let t = e.day_ticks.rem_euclid(24_000);
            if !self.day_times.iter().any(|r| r.contains(t)) {
                return false;
            }
        }
        true
    }
}

/// One alternate texture for a vanilla entity texture.
#[derive(Clone, Debug)]
pub struct Variant {
    /// The properties' 1-based rule index. Rewo uses it as the runtime
    /// variant id, so 0 always means "the vanilla texture".
    pub index: u32,
    /// Path inside the pack zip, or `None` when the rule names the vanilla
    /// texture itself.
    ///
    /// Packs conventionally list the original as `textures.1=<its own
    /// path>` or the MCPatcher-era `skins.1=1`, so that it takes a share of
    /// the weighting like any other alternate. Dropping such a rule for
    /// "having no texture" would quietly hand its share to the alternates
    /// and make every cow a variant cow — so it is kept, and drawing it
    /// simply means variant 0.
    pub texture: Option<String>,
    pub weight: u32,
    pub conditions: Conditions,
}

/// What [`EtfPack::pick`] needs to know about an entity. Everything here is
/// something Rewo actually decodes.
#[derive(Clone, Copy, Debug)]
pub struct EntityProps<'a> {
    /// The entity's UUID — the stable key the choice hashes.
    pub uuid: u128,
    /// Custom name from metadata, if any.
    pub name: Option<&'a str>,
    pub baby: bool,
    /// Slime / magma-cube size, if this is one.
    pub size: Option<i32>,
    /// Block Y, for `heights`.
    pub y: i32,
    /// World time in ticks, for `dayTime` and `moonPhase`.
    pub day_ticks: i64,
}

impl Default for EntityProps<'_> {
    fn default() -> Self {
        EntityProps { uuid: 0, name: None, baby: false, size: None, y: 64, day_ticks: 0 }
    }
}

/// OptiFine's emissive suffix. A pack may override it in
/// `optifine/emissive.properties` (`suffix.emissive=<s>`); `_e` is the
/// default and what every pack in practice uses.
const DEFAULT_EMISSIVE_SUFFIX: &str = "_e";

/// One alternate texture's pixels, ready for the entity atlas.
pub struct EtfTexture {
    /// The mob-texture key this varies (a key from `MOB_TEXTURE_SPECS`).
    pub key: &'static str,
    /// Variant id, matching [`Variant::index`].
    pub index: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// A pack's random-entity rules, keyed by the mob-texture keys the entity
/// pass uses. Empty (the [`Default`]) means every entity uses its vanilla
/// texture, which is what a pack-less run gets.
#[derive(Default)]
pub struct EtfPack {
    pub rules: HashMap<&'static str, Vec<Variant>>,
    pub textures: Vec<EtfTexture>,
    /// Emissive overlays: a mob texture that has a `<name>_e.png` sibling
    /// in the pack renders it as an always-fullbright layer over the whole
    /// model, which is what OptiFine's emissive textures are. Keyed by mob
    /// texture key; the pixels ride in `textures` under [`EMISSIVE_INDEX`].
    pub emissive: Vec<&'static str>,
}

/// The variant id reserved for a texture's emissive overlay. Real variant
/// ids come from the properties' 1-based rule indices, and this sits far
/// above any of them so the two can share the atlas machinery without
/// colliding.
pub const EMISSIVE_INDEX: u32 = 1 << 16;

impl EtfPack {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Choose a variant id for one entity: 0 for the vanilla texture, else
    /// a [`Variant::index`].
    ///
    /// Rules whose conditions all hold enter a weighted draw keyed on the
    /// entity's UUID, so the same entity always draws the same variant and
    /// a herd does not move in lockstep. No rule matching means the vanilla
    /// texture, which is also OptiFine's fallback.
    pub fn pick(&self, key: &str, e: &EntityProps<'_>) -> u32 {
        let Some(variants) = self.rules.get(key) else { return 0 };
        let matching: Vec<&Variant> = variants.iter().filter(|v| v.conditions.matches(e)).collect();
        let total: u64 = matching.iter().map(|v| v.weight.max(1) as u64).sum();
        if total == 0 {
            return 0;
        }
        let mut roll = (entity_hash(e.uuid) % total) as i64;
        for v in matching {
            roll -= v.weight.max(1) as i64;
            if roll < 0 {
                // A rule naming the vanilla texture draws normally and then
                // resolves to "no variant".
                return if v.texture.is_some() { v.index } else { 0 };
            }
        }
        0
    }
}

/// The entity → variant hash. **Ours, not OptiFine's** (see the module
/// docs): two splitmix64 rounds over the UUID halves, mixed together. What
/// it has to be is deterministic per entity and uniform across the weight
/// total; what it is not is bit-compatible with any other client.
fn entity_hash(uuid: u128) -> u64 {
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    mix(uuid as u64) ^ mix((uuid >> 64) as u64).rotate_left(32)
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Read a pack zip's random-entity properties and the alternate textures
/// they name. Entries that don't correspond to a vanilla entity texture
/// Rewo bakes, or whose alternates aren't the same size as the vanilla one,
/// are skipped with a notice — never an error, so a pack that only partly
/// applies still loads.
///
/// The size rule is Rewo's, not OptiFine's: the entity atlas gives a
/// variant the *same* UV rectangle as the texture it replaces, so a
/// differently-sized alternate cannot be addressed. Vanilla entity packs
/// keep the sizes anyway.
pub fn load_pack(zip_path: &Path) -> Result<EtfPack, String> {
    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("open pack {}: {e}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("pack zip {}: {e}", zip_path.display()))?;

    // Every file in the zip, so texture references can be resolved (and
    // probed for the `_e` emissive suffix) without re-walking it.
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_string()))
        .collect();

    let mut pack = EtfPack::default();
    let mut wanted: Vec<(&'static str, u32, String)> = Vec::new(); // key, index, zip path
    for props_path in names.iter().filter(|n| n.ends_with(".properties")) {
        let Some(rest) = PROPS_DIRS.iter().find_map(|d| props_path.strip_prefix(d)) else {
            continue;
        };
        // `optifine/random/entity/cow/cow.properties` describes
        // `textures/entity/cow/cow.png`.
        let base_rel = format!("entity/{}.png", rest.trim_end_matches(".properties"));
        let Some(key) = crate::assets::mob_texture_key(&base_rel) else {
            log::info!("etf: {props_path} has no matching baked mob texture ({base_rel}) — skipped");
            continue;
        };
        let mut text = String::new();
        let ok = zip
            .by_name(props_path)
            .ok()
            .and_then(|mut e| e.read_to_string(&mut text).ok())
            .is_some();
        if !ok {
            continue;
        }
        let base_dir = base_rel.rsplit_once('/').map_or("", |(d, _)| d);
        let base_stem = base_rel
            .rsplit('/')
            .next()
            .and_then(|f| f.strip_suffix(".png"))
            .unwrap_or("");
        let variants = parse_properties(&text, |r| {
            resolve_texture(r, base_dir, base_stem, props_path, &names)
        });
        if variants.is_empty() {
            continue;
        }
        for v in &variants {
            if let Some(path) = &v.texture {
                wanted.push((key, v.index, path.clone()));
            }
        }
        pack.rules.insert(key, variants);
    }

    // Decode the alternates, dropping any that don't match the vanilla
    // texture's dimensions.
    for (key, index, path) in wanted {
        let Some((bw, bh)) = crate::assets::mob_texture_size(key) else { continue };
        let Ok(mut entry) = zip.by_name(&path) else {
            log::warn!("etf: {key} variant {index}: {path} missing from the pack");
            continue;
        };
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() {
            continue;
        }
        match crate::assets::decode_entity_png(&buf) {
            Some((rgba, w, h)) if (w, h) == (bw, bh) => {
                pack.textures.push(EtfTexture { key, index, w, h, rgba })
            }
            Some((_, w, h)) => log::warn!(
                "etf: {key} variant {index} ({path}) is {w}×{h} but the vanilla texture is {bw}×{bh} — skipped"
            ),
            None => log::warn!("etf: {key} variant {index} ({path}) failed to decode"),
        }
    }
    // Emissive overlays: any baked mob texture with an `<name>_e.png` in
    // the pack. Independent of the random-entity rules — a pack may ship
    // emissive textures and no variants at all.
    let suffix = emissive_suffix(&mut zip, &names);
    for (key, base_rel, bw, bh) in crate::assets::mob_texture_specs() {
        let Some(stem) = base_rel.strip_suffix(".png") else { continue };
        let path = format!("{TEXTURES_ROOT}{stem}{suffix}.png");
        if !names.iter().any(|n| *n == path) {
            continue;
        }
        let Ok(mut entry) = zip.by_name(&path) else { continue };
        let mut buf = Vec::new();
        if entry.read_to_end(&mut buf).is_err() {
            continue;
        }
        match crate::assets::decode_entity_png(&buf) {
            Some((rgba, w, h)) if (w, h) == (bw, bh) => {
                pack.textures.push(EtfTexture { key, index: EMISSIVE_INDEX, w, h, rgba });
                pack.emissive.push(key);
            }
            Some((_, w, h)) => log::warn!(
                "etf: emissive {path} is {w}×{h} but {key} is {bw}×{bh} — skipped"
            ),
            None => log::warn!("etf: emissive {path} failed to decode"),
        }
    }

    // A rule whose image failed to load would return an id the atlas has no
    // slot for, so drop it — but keep the ones that name the vanilla
    // texture, which have no image by design.
    for (key, variants) in pack.rules.iter_mut() {
        variants.retain(|v| {
            v.texture.is_none() || pack.textures.iter().any(|t| t.key == *key && t.index == v.index)
        });
    }
    pack.rules.retain(|_, v| !v.is_empty());
    log::info!(
        "etf: pack {} → {} textures with variants, {} alternate images, {} emissive overlays",
        zip_path.display(),
        pack.rules.len(),
        pack.textures.len(),
        pack.emissive.len()
    );
    Ok(pack)
}

/// The pack's emissive suffix, from `optifine/emissive.properties`.
fn emissive_suffix(zip: &mut zip::ZipArchive<impl std::io::Read + std::io::Seek>, names: &[String]) -> String {
    const PROPS: &str = "assets/minecraft/optifine/emissive.properties";
    if !names.iter().any(|n| n == PROPS) {
        return DEFAULT_EMISSIVE_SUFFIX.to_string();
    }
    let mut text = String::new();
    if zip.by_name(PROPS).ok().and_then(|mut e| e.read_to_string(&mut text).ok()).is_none() {
        return DEFAULT_EMISSIVE_SUFFIX.to_string();
    }
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("suffix.emissive="))
        .map(|s| s.trim().to_string())
        .next()
        .unwrap_or_else(|| DEFAULT_EMISSIVE_SUFFIX.to_string())
}

/// Resolve one `textures.N` / `skins.N` value to a path inside the pack.
///
/// OptiFine accepts several spellings, in rough order of how packs use
/// them: a bare number (the MCPatcher form, meaning `<base><N>.png` beside
/// the vanilla texture), a bare filename (beside the vanilla texture), an
/// `assets/minecraft`-relative path introduced by `~/` or `/`, and a path
/// relative to the properties file itself.
fn resolve_texture(
    raw: &str,
    base_dir: &str,
    base_stem: &str,
    props_path: &str,
    names: &[String],
) -> Option<Option<String>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let vanilla = format!("{TEXTURES_ROOT}{base_dir}/{base_stem}.png");
    let with_png = |s: String| if s.ends_with(".png") { s } else { s + ".png" };
    let candidates: Vec<String> = if let Ok(n) = raw.parse::<u32>() {
        // `skins.2=2` → `<base><2>.png`; `=1` conventionally means the
        // vanilla texture itself.
        let suffix = if n == 1 { String::new() } else { n.to_string() };
        vec![format!("{TEXTURES_ROOT}{base_dir}/{base_stem}{suffix}.png")]
    } else if let Some(r) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix('/')) {
        vec![with_png(format!("assets/minecraft/{r}"))]
    } else if raw.contains('/') {
        // Ambiguous in the wild: try the properties file's directory first,
        // then the assets root.
        let props_dir = props_path.rsplit_once('/').map_or("", |(d, _)| d);
        vec![
            with_png(format!("{props_dir}/{raw}")),
            with_png(format!("assets/minecraft/{raw}")),
        ]
    } else {
        vec![with_png(format!("{TEXTURES_ROOT}{base_dir}/{raw}"))]
    };
    // A reference to the vanilla texture resolves even though the pack
    // doesn't contain it — in Minecraft the name is a resource location,
    // looked up through the pack stack down to the jar.
    if candidates.iter().any(|c| *c == vanilla) {
        return Some(None);
    }
    candidates
        .into_iter()
        .find(|c| names.iter().any(|n| n == c))
        .map(Some)
}

/// Parse a `random_entities.properties` body into variants, resolving each
/// texture reference through `resolve`. Rules whose texture can't be
/// resolved are dropped.
fn parse_properties(
    text: &str,
    mut resolve: impl FnMut(&str) -> Option<Option<String>>,
) -> Vec<Variant> {
    // index → (raw texture, weight, conditions)
    let mut raw: HashMap<u32, (Option<String>, u32, Conditions)> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        // `nbt.<index>.<path>` carries the index in the middle, unlike
        // every other key, which ends with it.
        let (name, idx) = match k.strip_prefix("nbt.") {
            Some(rest) => ("nbt", rest.split('.').next().unwrap_or("")),
            None => match k.rsplit_once('.') {
                Some((n, i)) => (n, i),
                None => continue,
            },
        };
        let Ok(idx) = idx.parse::<u32>() else { continue };
        let slot = raw.entry(idx).or_insert_with(|| (None, 1, Conditions::default()));
        match name {
            "textures" | "skins" => slot.0 = Some(v.to_string()),
            "weights" => slot.1 = v.parse().unwrap_or(1),
            "names" => slot.2.names = parse_names(v),
            "baby" => slot.2.baby = parse_bool(v),
            "sizes" => set_ranges(&mut slot.2.sizes, &mut slot.2.unsupported, v, "sizes"),
            "heights" => set_ranges(&mut slot.2.heights, &mut slot.2.unsupported, v, "heights"),
            "minHeight" => push_open(&mut slot.2.heights, v, true),
            "maxHeight" => push_open(&mut slot.2.heights, v, false),
            "moonPhase" => set_ranges(&mut slot.2.moon_phases, &mut slot.2.unsupported, v, "moonPhase"),
            "dayTime" => set_ranges(&mut slot.2.day_times, &mut slot.2.unsupported, v, "dayTime"),
            // Everything Rewo has no data for. The rule is kept (so the
            // weighting of the others is unchanged) but can never match.
            "biomes" | "professions" | "collarColors" | "health" | "weather" | "blocks" => {
                slot.2.unsupported = Some(leak_name(name))
            }
            "nbt" => slot.2.unsupported = Some("nbt"),
            _ => {}
        }
    }
    let mut out: Vec<Variant> = raw
        .into_iter()
        .filter_map(|(index, (tex, weight, conditions))| {
            let texture = resolve(tex.as_deref()?)?;
            Some(Variant { index, texture, weight, conditions })
        })
        .collect();
    out.sort_by_key(|v| v.index);
    out
}

/// The unsupported-condition names are a closed set, so this maps to
/// `'static` without leaking.
fn leak_name(name: &str) -> &'static str {
    match name {
        "biomes" => "biomes",
        "professions" => "professions",
        "collarColors" => "collarColors",
        "health" => "health",
        "weather" => "weather",
        "blocks" => "blocks",
        _ => "unknown",
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_names(v: &str) -> Vec<NameMatch> {
    v.split_whitespace()
        .map(|t| {
            if let Some(p) = t.strip_prefix("ipattern:") {
                NameMatch::Pattern { glob: p.to_string(), ignore_case: true }
            } else if let Some(p) = t.strip_prefix("pattern:") {
                NameMatch::Pattern { glob: p.to_string(), ignore_case: false }
            } else {
                NameMatch::Exact(t.to_string())
            }
        })
        .collect()
}

/// Parse a range list. A token we can't read makes the whole rule
/// unsupported rather than silently widening what it matches.
fn set_ranges(dst: &mut Vec<IntRange>, bad: &mut Option<Unsupported>, v: &str, what: &'static str) {
    for tok in v.split([',', ' ']).filter(|t| !t.trim().is_empty()) {
        match IntRange::parse(tok) {
            Some(r) => dst.push(r),
            None => {
                *bad = Some(what);
                return;
            }
        }
    }
}

fn push_open(dst: &mut Vec<IntRange>, v: &str, is_min: bool) {
    if let Ok(n) = v.trim().parse::<i64>() {
        // `minHeight`/`maxHeight` are the older half-open spelling; a pair
        // of them intersects, so fold into the single range already there.
        let r = if is_min {
            IntRange { lo: n, hi: i64::MAX }
        } else {
            IntRange { lo: i64::MIN, hi: n }
        };
        match dst.first_mut() {
            Some(f) => *f = IntRange { lo: f.lo.max(r.lo), hi: f.hi.min(r.hi) },
            None => dst.push(r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve stub: every reference resolves to a pack file of the same
    /// name, except the literal "vanilla", which stands for a reference to
    /// the base texture.
    fn ident(r: &str) -> Option<Option<String>> {
        Some((r != "vanilla").then(|| r.to_string()))
    }

    #[test]
    fn parses_a_plain_variant_list() {
        let v = parse_properties(
            "# a comment\ntextures.1=cow.png\ntextures.2=cow2.png\nweights.2=5\n",
            ident,
        );
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].index, 1);
        assert_eq!(v[0].weight, 1, "weights default to 1");
        assert_eq!(v[1].texture.as_deref(), Some("cow2.png"));
        assert_eq!(v[1].weight, 5);
    }

    #[test]
    fn a_rule_without_a_texture_is_dropped() {
        // Conditions on an index that never names a texture are meaningless.
        let v = parse_properties("weights.3=9\nbaby.3=true\ntextures.1=a.png\n", ident);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].index, 1);
    }

    #[test]
    fn parses_conditions() {
        let v = parse_properties(
            "textures.2=b.png\nnames.2=Bessie ipattern:moo*\nbaby.2=true\nsizes.2=1-3\n\
             heights.2=-60\nmoonPhase.2=0,4\ndayTime.2=13000-23000\n",
            ident,
        );
        let c = &v[0].conditions;
        assert_eq!(c.baby, Some(true));
        assert_eq!(c.sizes, vec![IntRange { lo: 1, hi: 3 }]);
        assert_eq!(c.heights, vec![IntRange { lo: -60, hi: -60 }], "a bare -60 is a value");
        assert_eq!(c.moon_phases.len(), 2);
        assert_eq!(c.day_times, vec![IntRange { lo: 13_000, hi: 23_000 }]);
        assert!(c.unsupported.is_none());
        assert_eq!(c.names.len(), 2);
    }

    #[test]
    fn ranges_cover_every_documented_spelling() {
        assert_eq!(IntRange::parse("5"), Some(IntRange { lo: 5, hi: 5 }));
        assert_eq!(IntRange::parse("-5"), Some(IntRange { lo: -5, hi: -5 }));
        assert_eq!(IntRange::parse("2-7"), Some(IntRange { lo: 2, hi: 7 }));
        assert_eq!(IntRange::parse("3-").map(|r| r.lo), Some(3));
        assert!(IntRange::parse("3-").unwrap().contains(1_000_000));
        assert!(IntRange::parse("").is_none());
        assert!(IntRange::parse("7-2").is_none(), "an inverted range is a typo");
        // Negative bounds: the separator is a later dash than the sign.
        assert_eq!(IntRange::parse("-64--30"), Some(IntRange { lo: -64, hi: -30 }));
        assert_eq!(IntRange::parse("-64-30"), Some(IntRange { lo: -64, hi: 30 }));
        assert_eq!(IntRange::parse("-64-").map(|r| r.lo), Some(-64));
        assert!(IntRange::parse("-64--30").unwrap().contains(-50));
        assert!(!IntRange::parse("-64--30").unwrap().contains(0));
        // `--30` (an open range to a negative bound) has no unambiguous
        // reading, so it is refused rather than guessed.
        assert!(IntRange::parse("--30").is_none());
    }

    #[test]
    fn wildcards_match_like_optifine_patterns() {
        assert!(glob_match("moo*", "moomoo"));
        assert!(glob_match("*cow*", "a cow here"));
        assert!(glob_match("c?w", "cow"));
        assert!(!glob_match("c?w", "coow"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("cow", "cows"));
        let m = NameMatch::Pattern { glob: "MOO*".into(), ignore_case: true };
        assert!(m.matches("moomoo"));
        assert!(!NameMatch::Pattern { glob: "MOO*".into(), ignore_case: false }.matches("moomoo"));
    }

    /// The load-bearing safety rule: a condition Rewo can't evaluate must
    /// make its rule inert, so the entity keeps its vanilla texture.
    #[test]
    fn unevaluatable_conditions_never_match() {
        for key in ["biomes", "professions", "collarColors", "health", "weather", "blocks"] {
            let v = parse_properties(&format!("textures.2=b.png\n{key}.2=whatever\n"), ident);
            assert_eq!(v[0].conditions.unsupported, Some(key), "{key}");
            assert!(!v[0].conditions.matches(&EntityProps::default()));
        }
        let v = parse_properties("textures.2=b.png\nnbt.2.Tags=x\n", ident);
        assert_eq!(v[0].conditions.unsupported, Some("nbt"));
    }

    fn pack_of(props: &str) -> EtfPack {
        let mut p = EtfPack::default();
        p.rules.insert("cow", parse_properties(props, ident));
        p
    }

    #[test]
    fn an_unmatched_or_unknown_texture_falls_back_to_vanilla() {
        let pack = pack_of("textures.2=b.png\nbaby.2=true\n");
        // Not a baby → no rule matches → the vanilla texture.
        assert_eq!(pack.pick("cow", &EntityProps { uuid: 7, ..Default::default() }), 0);
        // A texture with no rules at all.
        assert_eq!(pack.pick("pig", &EntityProps::default()), 0);
        // A baby draws the only matching rule, whatever its uuid.
        for uuid in 0..50u128 {
            let e = EntityProps { uuid, baby: true, ..Default::default() };
            assert_eq!(pack.pick("cow", &e), 2);
        }
    }

    /// The two properties the choice function has to have, since it can't
    /// be OptiFine's exact one: same entity → same variant, and the spread
    /// follows the weights.
    #[test]
    fn the_pick_is_stable_per_entity_and_follows_weights() {
        let pack = pack_of("textures.1=a.png\ntextures.2=b.png\nweights.1=1\nweights.2=3\n");
        let e = |uuid| EntityProps { uuid, ..Default::default() };
        for uuid in [0u128, 1, 12345, u128::MAX] {
            let first = pack.pick("cow", &e(uuid));
            for _ in 0..5 {
                assert_eq!(pack.pick("cow", &e(uuid)), first, "unstable for {uuid}");
            }
        }
        let n = 20_000u128;
        let twos = (0..n).filter(|u| pack.pick("cow", &e(*u)) == 2).count() as f64;
        let share = twos / n as f64;
        assert!(
            (share - 0.75).abs() < 0.02,
            "weight 3:1 should give variant 2 about 75% of entities, got {share}"
        );
    }

    /// The bug the render gate caught: a rule naming the vanilla texture
    /// (`textures.1=cow.png`, or the MCPatcher `skins.1=1`) is how packs
    /// give the *original* a share of the weighting. Dropping it as
    /// "textureless" would hand that share to the alternates, so every cow
    /// would be a variant cow.
    #[test]
    fn a_rule_naming_the_vanilla_texture_keeps_its_share() {
        let pack = pack_of("textures.1=vanilla
textures.2=b.png
");
        assert_eq!(pack.rules["cow"].len(), 2, "both rules survive parsing");
        assert!(pack.rules["cow"][0].texture.is_none(), "rule 1 is the vanilla texture");
        let n = 20_000u128;
        let vanilla = (0..n)
            .filter(|u| pack.pick("cow", &EntityProps { uuid: *u, ..Default::default() }) == 0)
            .count() as f64;
        let share = vanilla / n as f64;
        assert!(
            (share - 0.5).abs() < 0.02,
            "an even 1:1 split should leave half the herd vanilla, got {share}"
        );
    }

    /// The reserved emissive id is mirrored in `rewo_gpu::entities` (which
    /// can't depend on this crate), so pin it here — a drift would send a
    /// pack's emissive overlay in as an ordinary variant.
    #[test]
    fn the_emissive_index_sits_above_every_real_rule_index() {
        assert_eq!(EMISSIVE_INDEX, 1 << 16);
        // Rule indices come from `<key>.<n>` and are parsed as u32, but a
        // properties file with 65536 alternates is not a thing; the point is
        // that ordinary packs cannot reach it.
        let v = parse_properties("textures.9999=b.png
", ident);
        assert!(v[0].index < EMISSIVE_INDEX);
    }

    #[test]
    fn moon_phase_and_day_time_read_the_world_clock() {
        let pack = pack_of("textures.2=b.png\nmoonPhase.2=3\n");
        // Day 3 (ticks 72000..95999) is moon phase 3.
        let at = |t| EntityProps { uuid: 1, day_ticks: t, ..Default::default() };
        assert_eq!(pack.pick("cow", &at(72_000)), 2);
        assert_eq!(pack.pick("cow", &at(95_999)), 2);
        assert_eq!(pack.pick("cow", &at(96_000)), 0, "day 4 is phase 4");
        // Phases wrap every 8 days.
        assert_eq!(pack.pick("cow", &at(72_000 + 8 * 24_000)), 2);

        let night = pack_of("textures.2=b.png\ndayTime.2=13000-23000\n");
        assert_eq!(night.pick("cow", &at(0)), 0);
        assert_eq!(night.pick("cow", &at(18_000)), 2);
        assert_eq!(night.pick("cow", &at(24_000 + 18_000)), 2, "time of day wraps");
    }

    #[test]
    fn height_and_size_conditions() {
        let deep = pack_of("textures.2=b.png\nheights.2=-64--30\n");
        let at_y = |y| EntityProps { uuid: 1, y, ..Default::default() };
        assert_eq!(deep.pick("cow", &at_y(-40)), 2);
        assert_eq!(deep.pick("cow", &at_y(10)), 0);

        let big = pack_of("textures.2=b.png\nsizes.2=4-\n");
        let sized = |s| EntityProps { uuid: 1, size: s, ..Default::default() };
        assert_eq!(big.pick("cow", &sized(Some(4))), 2);
        assert_eq!(big.pick("cow", &sized(Some(2))), 0);
        assert_eq!(big.pick("cow", &sized(None)), 0, "a sizeless mob can't match a size rule");
    }

    #[test]
    fn min_and_max_height_intersect() {
        let v = parse_properties("textures.2=b.png\nminHeight.2=-20\nmaxHeight.2=40\n", ident);
        assert_eq!(v[0].conditions.heights, vec![IntRange { lo: -20, hi: 40 }]);
    }

    #[test]
    fn numeric_texture_references_use_the_mcpatcher_form() {
        let names: Vec<String> = ["assets/minecraft/textures/entity/cow/cow.png",
            "assets/minecraft/textures/entity/cow/cow2.png"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let r = |raw: &str| resolve_texture(raw, "entity/cow", "cow", "assets/minecraft/optifine/random/entity/cow/cow.properties", &names);
        let alt = Some(Some("assets/minecraft/textures/entity/cow/cow2.png".to_string()));
        assert_eq!(r("2"), alt);
        assert_eq!(r("cow2.png"), alt);
        assert_eq!(r("cow2"), alt, ".png is optional");
        assert_eq!(r("~/textures/entity/cow/cow2.png"), alt);
        // Naming the vanilla texture is a real rule, not a missing one —
        // `Some(None)` says "resolved, and it is the base texture".
        assert_eq!(r("1"), Some(None), "the MCPatcher `1` is the vanilla texture");
        assert_eq!(r("cow.png"), Some(None));
        assert_eq!(r("nope.png"), None, "a reference the pack doesn't contain");
    }
}
