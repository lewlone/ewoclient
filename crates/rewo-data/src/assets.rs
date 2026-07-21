//! M2 asset bake: client.jar → texture-array layers + per-state render table.
//!
//! Deliberately minimal (REWO_PLAN.md correction #2): only **full-cube**
//! blocks resolve — blockstate JSON → variant → model parent chain → either
//! a `cube`-family texture map or a model whose first element is the full
//! 16³ box (grass_block is defined that way). Everything else renders
//! Invisible in M2; the full model/quad baker is M4.
//!
//! Legality: textures are extracted at runtime from the user's own Mojang
//! download (the launcher's client jar); nothing ships in the binary.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::read_json_file;

pub const TEX_SIZE: u32 = 16;

/// Per-state render classification, indexed by global state id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderKind {
    Invisible,
    /// Texture-array layers per face: [up, down, north, south, west, east].
    Cube { faces: [u16; 6] },
}

pub struct BakedAssets {
    pub render: Vec<RenderKind>,
    /// RGBA8 16×16 texels per layer, straight from the pack (sRGB).
    pub layers: Vec<Vec<u8>>,
    pub layer_names: Vec<String>,
    pub stats: BakeStats,
}

#[derive(Default, Debug)]
pub struct BakeStats {
    pub cube_states: usize,
    pub invisible_states: usize,
    pub blocks_resolved: usize,
    pub blocks_skipped: usize,
    pub textures: usize,
}

/// Face order used across the mesher + render table.
pub const FACE_NAMES: [&str; 6] = ["up", "down", "north", "south", "west", "east"];

pub fn bake(client_jar: &Path, blocks_json: &Path) -> Result<BakedAssets, String> {
    let file = std::fs::File::open(client_jar)
        .map_err(|e| format!("open {}: {e}", client_jar.display()))?;
    let mut jar = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("zip {}: {e}", client_jar.display()))?;

    // blocks.json again, this time keeping per-state property maps (blocks.rs
    // intentionally stays lean; the bake needs them for variant matching).
    let blocks = read_json_file(blocks_json)?;
    let blocks = blocks.as_object().ok_or("blocks.json: not an object")?;
    let mut max_id = 0usize;
    for def in blocks.values() {
        if let Some(states) = def.get("states").and_then(|s| s.as_array()) {
            for s in states {
                if let Some(id) = s.get("id").and_then(|i| i.as_u64()) {
                    max_id = max_id.max(id as usize);
                }
            }
        }
    }

    let mut baker = Baker {
        jar: &mut jar,
        model_cache: HashMap::new(),
        layer_index: HashMap::new(),
        layers: Vec::new(),
        layer_names: Vec::new(),
    };

    let mut render = vec![RenderKind::Invisible; max_id + 1];
    let mut stats = BakeStats::default();

    for (block_name, def) in blocks {
        let states = def
            .get("states")
            .and_then(|s| s.as_array())
            .ok_or_else(|| format!("blocks.json: {block_name} has no states"))?;
        let short = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        let variants = baker.load_blockstate_variants(short);
        let mut any = false;
        for state in states {
            let Some(id) = state.get("id").and_then(|i| i.as_u64()) else {
                continue;
            };
            let props = state.get("properties").and_then(|p| p.as_object());
            let kind = variants
                .as_ref()
                .and_then(|v| pick_variant(v, props))
                .and_then(|model| baker.resolve_cube(&model));
            if let Some(faces) = kind {
                render[id as usize] = RenderKind::Cube { faces };
                stats.cube_states += 1;
                any = true;
            } else {
                stats.invisible_states += 1;
            }
        }
        if any {
            stats.blocks_resolved += 1;
        } else {
            stats.blocks_skipped += 1;
        }
    }

    stats.textures = baker.layers.len();
    log::info!(
        "rewo-data: baked {} cube states ({} blocks), {} invisible, {} textures",
        stats.cube_states,
        stats.blocks_resolved,
        stats.invisible_states,
        stats.textures
    );

    Ok(BakedAssets {
        render,
        layers: baker.layers,
        layer_names: baker.layer_names,
        stats,
    })
}

type Jar<'a> = &'a mut zip::ZipArchive<std::io::BufReader<std::fs::File>>;

struct Baker<'a> {
    jar: Jar<'a>,
    /// model path → (textures map, element faces if the model defines a
    /// full-size element).
    model_cache: HashMap<String, Option<ResolvedModel>>,
    layer_index: HashMap<String, u16>,
    layers: Vec<Vec<u8>>,
    layer_names: Vec<String>,
}

#[derive(Clone, Default)]
struct ResolvedModel {
    /// Merged texture variables (child overrides parent).
    textures: HashMap<String, String>,
    /// face name → texture ref (e.g. "#top"), from the first full-size
    /// element found anywhere in the parent chain.
    element_faces: Option<HashMap<String, String>>,
}

impl<'a> Baker<'a> {
    fn read_json(&mut self, path: &str) -> Option<serde_json::Value> {
        let mut entry = self.jar.by_name(path).ok()?;
        let mut text = String::new();
        entry.read_to_string(&mut text).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn load_blockstate_variants(&mut self, block: &str) -> Option<serde_json::Value> {
        let json = self.read_json(&format!("assets/minecraft/blockstates/{block}.json"))?;
        json.get("variants").cloned()
    }

    /// Resolve a model to 6 face texture layers, if it's a full cube.
    fn resolve_cube(&mut self, model: &str) -> Option<[u16; 6]> {
        let resolved = self.resolve_model(model)?;
        let element_faces = resolved.element_faces.as_ref()?;
        let mut faces = [0u16; 6];
        for (i, name) in FACE_NAMES.iter().enumerate() {
            let tex_ref = element_faces.get(*name)?;
            let tex_name = resolve_texture_var(tex_ref, &resolved.textures)?;
            faces[i] = self.layer_for(&tex_name)?;
        }
        Some(faces)
    }

    fn resolve_model(&mut self, model: &str) -> Option<ResolvedModel> {
        let key = model.strip_prefix("minecraft:").unwrap_or(model).to_string();
        if let Some(cached) = self.model_cache.get(&key) {
            return cached.clone();
        }
        let resolved = self.resolve_model_uncached(&key);
        self.model_cache.insert(key, resolved.clone());
        resolved
    }

    fn resolve_model_uncached(&mut self, key: &str) -> Option<ResolvedModel> {
        let json = self.read_json(&format!("assets/minecraft/models/{key}.json"))?;
        // Parent first (so child textures override), then merge this level.
        let mut out = match json.get("parent").and_then(|p| p.as_str()) {
            Some(parent) => self.resolve_model(parent)?,
            None => ResolvedModel::default(),
        };
        if let Some(textures) = json.get("textures").and_then(|t| t.as_object()) {
            for (var, value) in textures {
                if let Some(v) = value.as_str() {
                    out.textures.insert(var.clone(), v.to_string());
                }
            }
        }
        // First full-size element wins (grass_block's overlay elements and
        // partial-cube models are ignored by design in M2).
        if out.element_faces.is_none() {
            if let Some(faces) = full_cube_faces(&json) {
                out.element_faces = Some(faces);
            }
        }
        Some(out)
    }

    /// Get (or load + register) the texture-array layer for a texture name
    /// like "minecraft:block/dirt".
    fn layer_for(&mut self, tex_name: &str) -> Option<u16> {
        if let Some(&layer) = self.layer_index.get(tex_name) {
            return Some(layer);
        }
        let short = tex_name.strip_prefix("minecraft:").unwrap_or(tex_name);
        let path = format!("assets/minecraft/textures/{short}.png");
        let mut bytes = Vec::new();
        self.jar.by_name(&path).ok()?.read_to_end(&mut bytes).ok()?;
        let mut rgba = decode_png_rgba(&bytes)?;

        // Placeholder biome tint (M4 does it properly): the grass top ships
        // grayscale and expects a colormap multiply.
        if short.ends_with("grass_block_top") {
            tint(&mut rgba, [145, 189, 89]);
        }

        let layer = self.layers.len() as u16;
        self.layers.push(rgba);
        self.layer_names.push(tex_name.to_string());
        self.layer_index.insert(tex_name.to_string(), layer);
        Some(layer)
    }
}

/// If this model JSON's first element spans the full 16³ box with all six
/// faces, return face → texture-ref.
fn full_cube_faces(json: &serde_json::Value) -> Option<HashMap<String, String>> {
    let elements = json.get("elements")?.as_array()?;
    let first = elements.first()?;
    let coords = |k: &str| -> Option<[f64; 3]> {
        let a = first.get(k)?.as_array()?;
        Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
    };
    if coords("from")? != [0.0, 0.0, 0.0] || coords("to")? != [16.0, 16.0, 16.0] {
        return None;
    }
    let faces = first.get("faces")?.as_object()?;
    let mut out = HashMap::new();
    for name in FACE_NAMES {
        let tex = faces.get(name)?.get("texture")?.as_str()?;
        out.insert(name.to_string(), tex.to_string());
    }
    Some(out)
}

/// Follow `#var` references through the textures map to a concrete name.
fn resolve_texture_var<'a>(
    mut tex_ref: &'a str,
    textures: &'a HashMap<String, String>,
) -> Option<String> {
    for _ in 0..8 {
        if let Some(var) = tex_ref.strip_prefix('#') {
            tex_ref = textures.get(var)?;
        } else {
            return Some(tex_ref.to_string());
        }
    }
    None
}

/// Pick the variant matching a state's properties. Variant keys are
/// "prop=val,prop=val" (empty = wildcard); values may be an object or an
/// array of random rotations (take the first).
fn pick_variant(
    variants: &serde_json::Value,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<String> {
    let map = variants.as_object()?;
    for (key, value) in map {
        if variant_matches(key, props) {
            let entry = if let Some(arr) = value.as_array() {
                arr.first()?
            } else {
                value
            };
            return entry.get("model")?.as_str().map(str::to_string);
        }
    }
    None
}

fn variant_matches(
    key: &str,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if key.is_empty() {
        return true;
    }
    let Some(props) = props else {
        return false;
    };
    key.split(',').all(|pair| {
        let Some((k, v)) = pair.split_once('=') else {
            return false;
        };
        props.get(k).and_then(|pv| pv.as_str()) == Some(v)
    })
}

/// Decode a PNG to 16×16 RGBA8. Wider textures are rejected; taller ones
/// (animation strips) keep their first 16×16 frame — animation ticking
/// arrives with fluids in M4.
fn decode_png_rgba(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Vanilla textures are mostly palette-indexed: EXPAND turns indexed →
    // RGB(A) and tRNS → alpha, so the match below only sees plain formats.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.width != TEX_SIZE || info.height < TEX_SIZE {
        return None;
    }
    let px = TEX_SIZE as usize;
    let mut rgba = vec![0u8; px * px * 4];
    match info.color_type {
        png::ColorType::Rgba => {
            rgba.copy_from_slice(&buf[..px * px * 4]);
        }
        png::ColorType::Rgb => {
            for i in 0..px * px {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&buf[i * 3..i * 3 + 3]);
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for i in 0..px * px {
                let g = buf[i * 2];
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = buf[i * 2 + 1];
            }
        }
        png::ColorType::Grayscale => {
            for i in 0..px * px {
                let g = buf[i];
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = 255;
            }
        }
        // Unreachable after normalize_to_color8 (indexed is expanded).
        png::ColorType::Indexed => return None,
    }
    Some(rgba)
}

fn tint(rgba: &mut [u8], color: [u8; 3]) {
    for px in rgba.chunks_exact_mut(4) {
        for c in 0..3 {
            px[c] = ((px[c] as u16 * color[c] as u16) / 255) as u8;
        }
    }
}
