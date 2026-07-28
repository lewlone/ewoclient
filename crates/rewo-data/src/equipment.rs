//! Armour layer definitions and their textures (M46).
//!
//! 26.x describes a worn piece in two halves. The **item** names an asset
//! (`Equippable.assetId()`, e.g. `minecraft:diamond`), which
//! [`crate::item_props_table::equip_asset`] carries because it lives in the
//! item's prototype rather than on the wire. The **asset** then names its
//! layers:
//!
//! ```text
//! assets/minecraft/equipment/diamond.json
//!   { "layers": { "humanoid":          [ { "texture": "minecraft:diamond" } ],
//!                 "humanoid_leggings": [ { "texture": "minecraft:diamond" } ],
//!                 "horse_body":        [ … ] } }
//! ```
//!
//! and each layer's texture is
//! `entity/equipment/<layer>/<texture>.png` — a **64x32** sheet in the classic
//! armour layout, not a 64x64 skin.
//!
//! # A layer type maps to a *list* (M47)
//!
//! `EquipmentClientInfo.Layer(textureId, Optional<Dyeable>, usePlayerTexture)`,
//! and a layer type holds a non-empty list of them. In the 26.2 jar only
//! **leather** has two — a dyeable base plus an untinted overlay — and only
//! leather carries `dyeable` at all (20 humanoid lists of one, 3 of two, all
//! three of them leather's). The list is read generally anyway, because the
//! per-layer rule below is what decides *whether a layer draws*, and hard-
//! coding "base plus optional overlay" would bury that rule in a shape.
//!
//! # Why two layer types and not four
//!
//! `HumanoidArmorLayer.usesInnerModel` is `slot == LEGS`, so the leggings get
//! `humanoid_leggings` and the helmet, chestplate and boots all share
//! `humanoid`. The split exists because the leggings sit *inside* the
//! chestplate — a thinner inflation on its own sheet — and not because there
//! is a texture per slot.
//!
//! Only these two are read. `horse_body`, `happy_ghast_body`, `llama_body`
//! and the saddles describe geometry Rewo does not render, and reading them
//! would build a table nothing could draw.

use std::collections::HashMap;
use std::path::Path;

/// Which of the two humanoid layer sets a slot draws from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArmorLayer {
    /// `humanoid` — helmet, chestplate, boots.
    Humanoid,
    /// `humanoid_leggings` — leggings only.
    Leggings,
}

impl ArmorLayer {
    pub fn dir(self) -> &'static str {
        match self {
            ArmorLayer::Humanoid => "humanoid",
            ArmorLayer::Leggings => "humanoid_leggings",
        }
    }
}

/// `DyedItemColor.LEATHER_COLOR` — the `color_when_undyed` every leather layer
/// in the vanilla jar declares. An undyed leather boot is therefore **brown**,
/// not the greyscale its sheet is authored in.
pub const LEATHER_COLOR: u32 = 0xA0_65_40;

/// One layer of a piece: which sheet, and how it is tinted.
#[derive(Clone, Debug)]
pub struct ArmorLayerDef {
    /// The atlas key of this layer's sheet.
    pub key: String,
    /// The `dyeable` field. `None` is **absent**, which is not the same as
    /// `Some(None)`: absent draws untinted always, `Some(None)` draws *only*
    /// when the stack is dyed (`Layer.onlyIfDyed`).
    pub dyeable: Option<Option<u32>>,
}

/// `EquipmentLayerRenderer.getColorForLayer`, verbatim.
///
/// ```java
/// Optional<Dyeable> dyeable = layer.dyeable();
/// if (dyeable.isPresent()) {
///    int colorWhenUndyed = dyeable.get().colorWhenUndyed().map(ARGB::opaque).orElse(0);
///    return dyeColor != 0 ? dyeColor : colorWhenUndyed;
/// } else {
///    return -1;
/// }
/// ```
///
/// **Zero means do not draw this layer at all** — the caller's guard is
/// `if (color != 0)`. That is the whole mechanism behind `Layer.onlyIfDyed`: a
/// `Dyeable` carrying no `color_when_undyed` returns 0 for an undyed stack and
/// the dye for a dyed one. `-1` is `0xFFFFFFFF`, white, which multiplies to
/// nothing.
pub fn color_for_layer(dyeable: Option<Option<u32>>, dye_argb: u32) -> u32 {
    match dyeable {
        Some(color_when_undyed) => {
            if dye_argb != 0 {
                dye_argb
            } else {
                // `ARGB::opaque` — the JSON value is an RGB.
                color_when_undyed.map_or(0, |c| 0xFF00_0000 | (c & 0x00FF_FFFF))
            }
        }
        None => 0xFFFF_FFFF,
    }
}

/// `DyedItemColor.getOrDefault(stack, 0)` — an item's dye as an opaque ARGB,
/// or **0** for "undyed". The component holds an RGB, so the alpha goes on
/// here rather than arriving on the wire.
pub fn dye_argb(dyed_color: Option<i32>) -> u32 {
    match dyed_color {
        Some(rgb) => 0xFF00_0000 | (rgb as u32 & 0x00FF_FFFF),
        None => 0,
    }
}

/// One decoded 64x32 armour sheet.
#[derive(Clone, Debug)]
pub struct ArmorTexture {
    /// `<asset>/<layer>`, the key the renderer's atlas is packed by.
    pub key: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Every armour asset's humanoid layers, and the sheets they name.
#[derive(Clone, Debug, Default)]
pub struct EquipmentAssets {
    /// `(asset, layer)` → that layer type's ordered layer list. Ordered
    /// because `renderLayers` draws them in order and the overlay must land on
    /// top of the base it covers.
    by_asset: HashMap<(String, ArmorLayer), Vec<ArmorLayerDef>>,
    pub textures: Vec<ArmorTexture>,
}

impl EquipmentAssets {
    /// Read every `assets/minecraft/equipment/*.json` and decode the humanoid
    /// sheets they name.
    ///
    /// An asset with no humanoid layer at all (a saddle, a harness) simply
    /// contributes nothing — it is not an error, because those assets are for
    /// geometry this client does not draw.
    pub fn load(client_jar: &Path) -> Self {
        let mut out = Self::default();
        let Ok(file) = std::fs::File::open(client_jar) else {
            return out;
        };
        let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
            return out;
        };
        let names: Vec<String> = zip
            .file_names()
            .filter(|p| p.starts_with("assets/minecraft/equipment/") && p.ends_with(".json"))
            .map(str::to_string)
            .collect();
        // `(asset, layer) -> texture name`, gathered before any decode so the
        // same sheet named by two assets is decoded once.
        #[allow(clippy::type_complexity)]
        let mut wanted: Vec<((String, ArmorLayer), String, Option<Option<u32>>)> = Vec::new();
        for path in &names {
            let Some(asset) = path
                .rsplit('/')
                .next()
                .and_then(|f| f.strip_suffix(".json"))
                .map(str::to_string)
            else {
                continue;
            };
            let mut raw = String::new();
            {
                let Ok(mut e) = zip.by_name(path) else { continue };
                if std::io::Read::read_to_string(&mut e, &mut raw).is_err() {
                    continue;
                }
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            for layer in [ArmorLayer::Humanoid, ArmorLayer::Leggings] {
                let Some(list) = json
                    .get("layers")
                    .and_then(|l| l.get(layer.dir()))
                    .and_then(|a| a.as_array())
                else {
                    continue;
                };
                for entry in list {
                    let Some(tex) = entry.get("texture").and_then(|t| t.as_str()) else {
                        continue;
                    };
                    // `Optional<Dyeable>` — absent, or present with an
                    // optional `color_when_undyed`. The three states are
                    // distinct and each renders differently, so they survive
                    // as `Option<Option<u32>>` rather than collapsing.
                    let dyeable = entry.get("dyeable").map(|d| {
                        d.get("color_when_undyed")
                            .and_then(|c| c.as_i64())
                            .map(|c| c as u32 & 0x00FF_FFFF)
                    });
                    let tex = tex.strip_prefix("minecraft:").unwrap_or(tex).to_string();
                    wanted.push(((format!("minecraft:{asset}"), layer), tex, dyeable));
                }
            }
        }
        for ((asset, layer), tex, dyeable) in wanted {
            // Keyed by the **texture**, not by the asset: two assets naming the
            // same sheet share one atlas entry, and one asset's two layers name
            // two different sheets.
            let key = format!("{}/{tex}", layer.dir());
            let path = format!(
                "assets/minecraft/textures/entity/equipment/{}/{tex}.png",
                layer.dir()
            );
            let mut bytes = Vec::new();
            let decoded = zip
                .by_name(&path)
                .ok()
                .and_then(|mut e| std::io::Read::read_to_end(&mut e, &mut bytes).ok())
                .and_then(|_| crate::assets::decode_png_any(&bytes));
            let Some((rgba, w, h)) = decoded else {
                continue;
            };
            if !out.textures.iter().any(|t| t.key == key) {
                out.textures.push(ArmorTexture { key: key.clone(), w, h, rgba });
            }
            out.by_asset
                .entry((asset, layer))
                .or_default()
                .push(ArmorLayerDef { key, dyeable });
        }
        log::info!(
            "rewo-data: {} armour sheet(s) over {} asset/layer pair(s), {} layer(s)",
            out.textures.len(),
            out.by_asset.len(),
            out.by_asset.values().map(Vec::len).sum::<usize>(),
        );
        out
    }

    /// An asset's layer list, in draw order. Empty for anything the jar does
    /// not describe with a humanoid layer.
    pub fn layers(&self, asset: &str, layer: ArmorLayer) -> &[ArmorLayerDef] {
        self.by_asset
            .get(&(asset.to_string(), layer))
            .map_or(&[], |v| v.as_slice())
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

// -- armour trims (M48) -------------------------------------------------------

/// `PalettedPermutations.createPaletteMapping` + `NativeImage.mappedCopy`.
///
/// The trim sprites do **not** exist as files. `assets/minecraft/atlases/
/// armor_trims.json` declares a `paletted_permutations` source, and the client
/// generates one sprite per (pattern x material) at load by swapping colours
/// through a palette pair:
///
/// ```java
/// for (int i = 0; i < keys.length; i++) {
///    int key = keys[i];
///    if (ARGB.alpha(key) != 0) palette.put(ARGB.transparent(key), values[i]);
/// }
/// return pixel -> {
///    int pixelAlpha = ARGB.alpha(pixel);
///    if (pixelAlpha == 0) return pixel;
///    int pixelRGB = ARGB.transparent(pixel);
///    int value = palette.getOrDefault(pixelRGB, ARGB.opaque(pixelRGB));
///    return ARGB.color(pixelAlpha * ARGB.alpha(value) / 255, value);
/// };
/// ```
///
/// Two things are easy to get wrong. The match is on **RGB with the alpha
/// masked off**, both when building the map and when looking up — so a
/// half-transparent pixel of a palette colour still maps. And an **unmatched**
/// pixel is not dropped: `getOrDefault` hands back `opaque(pixelRGB)`, whose
/// alpha is 255, so the pixel survives with its own alpha and colour intact.
///
/// Working in RGBA bytes rather than packed ints sidesteps the question of
/// whether `NativeImage.getPixels` is ARGB or ABGR: keys, values and source all
/// come from the same decoder, so a consistent channel order cancels.
pub fn apply_palette(src: &[u8], key: &[u8], value: &[u8]) -> Option<Vec<u8>> {
    if key.len() != value.len() {
        // Vanilla throws here — the two palettes must be the same length.
        return None;
    }
    let mut map: HashMap<[u8; 3], [u8; 4]> = HashMap::with_capacity(key.len() / 4);
    for i in (0..key.len()).step_by(4) {
        if key[i + 3] == 0 {
            continue;
        }
        map.insert(
            [key[i], key[i + 1], key[i + 2]],
            [value[i], value[i + 1], value[i + 2], value[i + 3]],
        );
    }
    let mut out = src.to_vec();
    for px in out.chunks_exact_mut(4) {
        let pa = px[3];
        if pa == 0 {
            continue;
        }
        match map.get(&[px[0], px[1], px[2]]) {
            Some(v) => {
                px[0] = v[0];
                px[1] = v[1];
                px[2] = v[2];
                // `pixelAlpha * valueAlpha / 255`, integer.
                px[3] = ((pa as u32 * v[3] as u32) / 255) as u8;
            }
            // `opaque(pixelRGB)` — alpha 255, so the pixel is unchanged.
            None => {}
        }
    }
    Some(out)
}

/// The trim source textures and palettes, read once from the jar (M48).
#[derive(Clone, Debug, Default)]
pub struct TrimAssets {
    /// `trims/entity/<layer>/<pattern>` → the greyscale source, RGBA.
    sources: HashMap<String, (Vec<u8>, u32, u32)>,
    /// The `palette_key` strip every permutation is matched against.
    key_palette: Vec<u8>,
    /// `<suffix>` → that material's palette strip.
    palettes: HashMap<String, Vec<u8>>,
}

impl TrimAssets {
    pub fn load(client_jar: &Path) -> Self {
        let mut out = Self::default();
        let Ok(file) = std::fs::File::open(client_jar) else {
            return out;
        };
        let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
            return out;
        };
        const SRC: &str = "assets/minecraft/textures/trims/entity/";
        const PAL: &str = "assets/minecraft/textures/trims/color_palettes/";
        let names: Vec<String> = zip
            .file_names()
            .filter(|p| (p.starts_with(SRC) || p.starts_with(PAL)) && p.ends_with(".png"))
            .map(str::to_string)
            .collect();
        for path in names {
            let mut bytes = Vec::new();
            let decoded = zip
                .by_name(&path)
                .ok()
                .and_then(|mut e| std::io::Read::read_to_end(&mut e, &mut bytes).ok())
                .and_then(|_| crate::assets::decode_png_any(&bytes));
            let Some((rgba, w, h)) = decoded else {
                continue;
            };
            if let Some(rest) = path.strip_prefix(PAL) {
                let name = rest.trim_end_matches(".png");
                if name == "trim_palette" {
                    out.key_palette = rgba;
                } else {
                    out.palettes.insert(name.to_string(), rgba);
                }
            } else if let Some(rest) = path.strip_prefix(SRC) {
                let name = rest.trim_end_matches(".png");
                out.sources
                    .insert(format!("trims/entity/{name}"), (rgba, w, h));
            }
        }
        log::info!(
            "rewo-data: {} trim source(s), {} palette(s)",
            out.sources.len(),
            out.palettes.len()
        );
        out
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() || self.key_palette.is_empty()
    }

    /// Generate one permuted sprite: `<prefix>/<pattern>_<suffix>`.
    ///
    /// Takes the two halves separately because the *source* is named without
    /// the material — `trims/entity/humanoid/coast` is one file, and the
    /// seventeen `coast_<material>` sprites are all made from it.
    pub fn permute(&self, source_path: &str, suffix: &str) -> Option<(Vec<u8>, u32, u32)> {
        let (src, w, h) = self.sources.get(source_path)?;
        let palette = self.palettes.get(suffix)?;
        let rgba = apply_palette(src, &self.key_palette, palette)?;
        Some((rgba, *w, *h))
    }
}
