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
    /// `(asset, layer)` → index into [`Self::textures`].
    by_asset: HashMap<(String, ArmorLayer), usize>,
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
        let mut wanted: Vec<((String, ArmorLayer), String)> = Vec::new();
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
                // **The first entry only.** A layer is a *list* — leather has
                // a dyeable base plus an overlay — and Rewo draws one sheet
                // per piece, so it takes the base and leaves the overlay. That
                // is why an undyed leather helmet looks right and a dyed one
                // is not tinted yet.
                let Some(tex) = json
                    .get("layers")
                    .and_then(|l| l.get(layer.dir()))
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|e| e.get("texture"))
                    .and_then(|t| t.as_str())
                else {
                    continue;
                };
                let tex = tex.strip_prefix("minecraft:").unwrap_or(tex).to_string();
                wanted.push(((format!("minecraft:{asset}"), layer), tex));
            }
        }
        for ((asset, layer), tex) in wanted {
            let key = format!("{asset}/{}", layer.dir());
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
            out.by_asset.insert((asset, layer), out.textures.len());
            out.textures.push(ArmorTexture { key, w, h, rgba });
        }
        log::info!(
            "rewo-data: {} armour sheet(s) over {} asset/layer pair(s)",
            out.textures.len(),
            out.by_asset.len()
        );
        out
    }

    /// The atlas key for an asset's layer, or `None` if the jar named none.
    pub fn key(&self, asset: &str, layer: ArmorLayer) -> Option<&str> {
        self.by_asset
            .get(&(asset.to_string(), layer))
            .map(|&i| self.textures[i].key.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}
