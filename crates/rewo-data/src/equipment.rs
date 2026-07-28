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
