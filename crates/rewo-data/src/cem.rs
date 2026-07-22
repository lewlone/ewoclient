//! OptiFine CEM (Custom Entity Models) resource-pack loading — the pack
//! side of the EMF-equivalent (REWO_PLAN §12 M9). This module only reads
//! the raw `.jem` / `.jpm` JSON strings out of a pack zip; the geometry
//! parse (JEM → model) lives in `rewo_gpu::cem`, next to the model
//! primitives it builds against.
//!
//! Layout inside a pack: `assets/minecraft/optifine/cem/<entity>.jem`
//! (the model), optionally referencing `<name>.jpm` part files by a
//! `"model"` field. We pull every `.jem` plus every `.jpm` so the parser
//! can resolve those references without touching the zip again.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// One entity's CEM override: the entity name (file stem, e.g. "creeper")
/// and its raw `.jem` JSON. `.jpm` files referenced by the `.jem` are
/// carried in the sibling map so the parser can resolve them.
pub struct CemFile {
    pub entity: String,
    pub jem: String,
}

/// Everything a pack contributes for CEM: the per-entity `.jem` files plus
/// every `.jpm` by filename (for `"model"` reference resolution).
pub struct CemPack {
    pub files: Vec<CemFile>,
    pub jpms: HashMap<String, String>,
}

const CEM_DIR: &str = "assets/minecraft/optifine/cem/";

/// Load every CEM `.jem` (+ `.jpm`) from a resource-pack zip. Returns an
/// empty pack (not an error) if the zip has no CEM directory — a pack
/// without custom models is valid.
pub fn load_pack(zip_path: &Path) -> Result<CemPack, String> {
    let file = std::fs::File::open(zip_path)
        .map_err(|e| format!("open pack {}: {e}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("pack zip {}: {e}", zip_path.display()))?;

    let mut files = Vec::new();
    let mut jpms = HashMap::new();
    for i in 0..zip.len() {
        let mut entry = match zip.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().to_string();
        let Some(rest) = name.strip_prefix(CEM_DIR) else { continue };
        // Only files directly in cem/ (jem/jpm); skip nested dirs + others.
        if rest.contains('/') {
            continue;
        }
        let is_jem = rest.ends_with(".jem");
        let is_jpm = rest.ends_with(".jpm");
        if !is_jem && !is_jpm {
            continue;
        }
        let mut text = String::new();
        if entry.read_to_string(&mut text).is_err() {
            continue;
        }
        if let Some(stem) = rest.strip_suffix(".jem") {
            files.push(CemFile { entity: stem.to_string(), jem: text });
        } else {
            jpms.insert(rest.to_string(), text);
        }
    }
    log::info!(
        "cem: pack {} → {} models, {} jpm parts",
        zip_path.display(),
        files.len(),
        jpms.len()
    );
    Ok(CemPack { files, jpms })
}
