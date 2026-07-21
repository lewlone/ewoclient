//! M4 asset bake: client.jar → texture-array layers + per-state render data.
//!
//! Unlike M2 (full cubes only), M4 resolves the **general block model**:
//! blockstate variants (with x/y rotation) or multipart (with `when`
//! conditions), model parent chains, arbitrary `elements` (from/to boxes),
//! per-face uv / texture / cullface / tintindex, element rotation
//! (origin/axis/angle/rescale), and `shade`/`ambientocclusion` flags. Output
//! is either a fast-path `Cube` (a full opaque 16³ box, kept for cheap
//! face-culling + AO) or a `Model` — a baked quad list.
//!
//! Deferred (documented, not done): uvlock (minor texture-rotation cosmetic),
//! true per-biome tint (a fixed plains color is baked from the colormap
//! centers — see `grass_tint`/`foliage_tint`), animated-texture frame
//! ticking, and greedy meshing (conflicts with per-vertex AO).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::read_json_file;

pub const TEX_SIZE: u32 = 16;

/// Face order used across the bake + mesher:
/// 0 up(+Y) 1 down(-Y) 2 north(-Z) 3 south(+Z) 4 west(-X) 5 east(+X).
pub const FACE_NAMES: [&str; 6] = ["up", "down", "north", "south", "west", "east"];
const FACE_NORMALS: [[f32; 3]; 6] = [
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, -1.0],
    [0.0, 0.0, 1.0],
    [-1.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
];

/// One textured quad in block-local space (0..1), baked from a model face.
#[derive(Clone, Debug)]
pub struct Quad {
    pub verts: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub layer: u16,
    /// Face index (0..5) this quad is culled against when that neighbor is a
    /// full opaque cube, or -1 to never cull.
    pub cull: i8,
    /// Face index for directional shading.
    pub dir: u8,
    /// Biome-tint this quad (grass/foliage color).
    pub tint: bool,
    /// Apply directional face shading (false for plants/torches).
    pub shade: bool,
}

/// Per-state render classification, indexed by global state id.
#[derive(Clone, Debug)]
pub enum RenderKind {
    Invisible,
    /// Full opaque 16³ cube — [up,down,north,south,west,east] layers + a
    /// per-face tint flag. Fast path (face-cull + AO in the mesher).
    Cube {
        faces: [u16; 6],
        tint: [bool; 6],
    },
    /// Water/lava — geometry comes from the mesher's fluid path (corner
    /// heights, not a model; vanilla hardcodes fluid rendering the same
    /// way). `level`: 0 = source, 1..7 = flowing, ≥8 = falling full block.
    /// Water goes to the translucent mesh (texture alpha 180), lava to the
    /// opaque mesh at full bright.
    Fluid {
        layer: u16,
        level: u8,
        lava: bool,
    },
    /// General model — index into `BakedAssets::models`.
    Model(u32),
}

pub struct BakedAssets {
    pub render: Vec<RenderKind>,
    pub models: Vec<Vec<Quad>>,
    /// Per-state full-cube collision flag — true for a `Cube` OR a `Model`
    /// with a full 16³ element (grass_block renders as a Model because of its
    /// overlay element, but collides as a solid cube). Drives the client's
    /// M3 full-cube collision, independent of the render fast-path.
    pub solid: Vec<bool>,
    /// RGBA8 16×16 texels per layer (sRGB).
    pub layers: Vec<Vec<u8>>,
    pub layer_names: Vec<String>,
    /// Baked plains-biome tint colors (colormap centers).
    pub grass_tint: [u8; 3],
    pub foliage_tint: [u8; 3],
    /// The vanilla bitmap font (ascii.png) for nametags/HUD text. `None`
    /// only if the jar somehow lacks it — callers degrade to no text.
    pub font: Option<BakedFont>,
    pub stats: BakeStats,
}

/// The legacy bitmap font: a 16×16 grid of glyph cells covering code points
/// 0..256 (ASCII names only need rows 2..8). Advances derived from the
/// bitmap the way vanilla's legacy provider does (rightmost lit column + 1
/// spacing px; space = 4).
pub struct BakedFont {
    /// RGBA8 atlas, `atlas_size`² texels (sRGB; white glyphs on alpha).
    pub atlas: Vec<u8>,
    pub atlas_size: u32,
    /// Glyph cell edge in px (vanilla: 8).
    pub cell: u32,
    /// Horizontal advance per code point, in font px.
    pub advance: [u8; 256],
    /// A guaranteed-opaque-white texel (patched into the blank space cell),
    /// for drawing solid quads through the same pipeline. (u, v) in texels.
    pub white_texel: (u32, u32),
}

#[derive(Default, Debug)]
pub struct BakeStats {
    pub cube_states: usize,
    pub model_states: usize,
    pub fluid_states: usize,
    pub invisible_states: usize,
    pub textures: usize,
}

pub fn bake(client_jar: &Path, blocks_json: &Path) -> Result<BakedAssets, String> {
    let file = std::fs::File::open(client_jar)
        .map_err(|e| format!("open {}: {e}", client_jar.display()))?;
    let mut jar = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("zip {}: {e}", client_jar.display()))?;

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

    let grass_tint = colormap_center(&mut jar, "grass").unwrap_or([124, 189, 107]);
    let foliage_tint = colormap_center(&mut jar, "foliage").unwrap_or([89, 174, 48]);
    let font = bake_font(&mut jar);
    if font.is_none() {
        log::warn!("rewo-data: font/ascii.png missing — nametags disabled");
    }

    let mut baker = Baker {
        jar: &mut jar,
        model_cache: HashMap::new(),
        layer_index: HashMap::new(),
        layers: Vec::new(),
        layer_names: Vec::new(),
        grass_tint,
        foliage_tint,
    };

    let mut render = vec![RenderKind::Invisible; max_id + 1];
    let mut solid = vec![false; max_id + 1];
    let mut models: Vec<Vec<Quad>> = Vec::new();
    let mut stats = BakeStats::default();

    for (block_name, def) in blocks {
        let states = def
            .get("states")
            .and_then(|s| s.as_array())
            .ok_or_else(|| format!("blocks.json: {block_name} has no states"))?;
        let short = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        // Fluids have no usable blockstate models (vanilla hardcodes their
        // renderer) — classify by name, keyed on the `level` property.
        if short == "water" || short == "lava" {
            let lava = short == "lava";
            let layer = if lava {
                baker.layer_for("block/lava_still", TintKind::None)
            } else {
                baker.layer_for("block/water_still", TintKind::Water)
            };
            let Some(layer) = layer else {
                log::warn!("rewo-data: {short}_still texture missing — fluid invisible");
                continue;
            };
            for state in states {
                let Some(id) = state.get("id").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let level = state
                    .get("properties")
                    .and_then(|p| p.get("level"))
                    .and_then(|l| l.as_str())
                    .and_then(|l| l.parse::<u8>().ok())
                    .unwrap_or(0);
                render[id as usize] = RenderKind::Fluid { layer, level, lava };
                stats.fluid_states += 1;
            }
            continue;
        }
        let bs = baker.load_blockstate(short);
        let foliage = is_foliage(short);
        for state in states {
            let Some(id) = state.get("id").and_then(|i| i.as_u64()) else {
                continue;
            };
            let props = state.get("properties").and_then(|p| p.as_object());
            let resolved = bs
                .as_ref()
                .and_then(|bs| baker.resolve_state(bs, props, foliage, &mut models));
            match resolved {
                Some((k @ RenderKind::Cube { .. }, is_solid)) => {
                    render[id as usize] = k;
                    solid[id as usize] = is_solid;
                    stats.cube_states += 1;
                }
                Some((k @ RenderKind::Model(_), is_solid)) => {
                    render[id as usize] = k;
                    solid[id as usize] = is_solid;
                    stats.model_states += 1;
                }
                _ => stats.invisible_states += 1,
            }
        }
    }

    stats.textures = baker.layers.len();
    log::info!(
        "rewo-data: baked {} cubes + {} models ({} invisible), {} textures",
        stats.cube_states,
        stats.model_states,
        stats.invisible_states,
        stats.textures
    );

    Ok(BakedAssets {
        render,
        solid,
        models,
        layers: baker.layers,
        layer_names: baker.layer_names,
        grass_tint,
        foliage_tint,
        font,
        stats,
    })
}

/// Extract + measure the legacy bitmap font from the jar.
fn bake_font(jar: Jar) -> Option<BakedFont> {
    let mut bytes = Vec::new();
    jar.by_name("assets/minecraft/textures/font/ascii.png")
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    if w != h || w % 16 != 0 {
        return None;
    }
    let px = w as usize;
    let mut atlas = vec![0u8; px * px * 4];
    match info.color_type {
        png::ColorType::Rgba => atlas.copy_from_slice(&buf[..px * px * 4]),
        png::ColorType::GrayscaleAlpha => {
            for i in 0..px * px {
                let g = buf[i * 2];
                atlas[i * 4..i * 4 + 3].copy_from_slice(&[g, g, g]);
                atlas[i * 4 + 3] = buf[i * 2 + 1];
            }
        }
        _ => return None,
    }
    let cell = w / 16;
    let advance = font_advances(&atlas, w, cell);

    // Patch one opaque-white texel into the space glyph's cell (guaranteed
    // blank — and text layout never emits quads for spaces, so it can't
    // show). Solid quads (nametag backgrounds, capsules) sample it.
    let (wx, wy) = ((32 % 16) * cell, (32 / 16) * cell);
    let wi = ((wy as usize) * px + wx as usize) * 4;
    atlas[wi..wi + 4].copy_from_slice(&[255, 255, 255, 255]);

    Some(BakedFont {
        atlas,
        atlas_size: w,
        cell,
        advance,
        white_texel: (wx, wy),
    })
}

/// Per-glyph advances: rightmost column with any alpha + 2 (1 px glyph edge
/// + 1 px spacing), matching vanilla's legacy provider; blank cells (space)
/// advance 4.
fn font_advances(atlas: &[u8], size: u32, cell: u32) -> [u8; 256] {
    let mut advance = [0u8; 256];
    for cp in 0..256u32 {
        let (cx, cy) = ((cp % 16) * cell, (cp / 16) * cell);
        let mut rightmost: Option<u32> = None;
        for col in 0..cell {
            for row in 0..cell {
                let i = (((cy + row) * size + cx + col) * 4 + 3) as usize;
                if atlas[i] > 0 {
                    rightmost = Some(col);
                    break;
                }
            }
        }
        advance[cp as usize] = match rightmost {
            Some(r) => (r + 2) as u8,
            None => 4,
        };
    }
    advance
}

type Jar<'a> = &'a mut zip::ZipArchive<std::io::BufReader<std::fs::File>>;

struct Baker<'a> {
    jar: Jar<'a>,
    model_cache: HashMap<String, Option<ResolvedModel>>,
    layer_index: HashMap<String, u16>,
    layers: Vec<Vec<u8>>,
    layer_names: Vec<String>,
    grass_tint: [u8; 3],
    foliage_tint: [u8; 3],
}

/// A model with its parent chain flattened: merged textures + all elements.
#[derive(Clone)]
struct ResolvedModel {
    textures: HashMap<String, String>,
    elements: Vec<serde_json::Value>,
    ambient_occlusion: bool,
}

/// A blockstate reference to a model with optional whole-model rotation.
struct ModelRef {
    model: String,
    x: i32,
    y: i32,
}

/// A parsed blockstate: variants or multipart.
enum BlockState {
    Variants(serde_json::Map<String, serde_json::Value>),
    Multipart(Vec<serde_json::Value>),
}

impl<'a> Baker<'a> {
    fn read_json(&mut self, path: &str) -> Option<serde_json::Value> {
        let mut entry = self.jar.by_name(path).ok()?;
        let mut text = String::new();
        entry.read_to_string(&mut text).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn load_blockstate(&mut self, block: &str) -> Option<BlockState> {
        let json = self.read_json(&format!("assets/minecraft/blockstates/{block}.json"))?;
        if let Some(v) = json.get("variants").and_then(|v| v.as_object()) {
            Some(BlockState::Variants(v.clone()))
        } else {
            json.get("multipart")
                .and_then(|m| m.as_array())
                .map(|m| BlockState::Multipart(m.clone()))
        }
    }

    /// Resolve one state's render data. Cube fast-path only for a single
    /// variant whose model is a full opaque cube; everything else bakes to a
    /// quad list.
    fn resolve_state(
        &mut self,
        bs: &BlockState,
        props: Option<&serde_json::Map<String, serde_json::Value>>,
        foliage: bool,
        models: &mut Vec<Vec<Quad>>,
    ) -> Option<(RenderKind, bool)> {
        let refs: Vec<ModelRef> = match bs {
            BlockState::Variants(v) => pick_variant(v, props).into_iter().collect(),
            BlockState::Multipart(parts) => parts
                .iter()
                .filter(|p| multipart_applies(p, props))
                .filter_map(|p| model_ref(p.get("apply")?))
                .collect(),
        };
        if refs.is_empty() {
            return None;
        }

        // Collision solidity: any referenced model with a full 16³ element
        // makes this a solid cube (grass_block renders as a Model due to its
        // overlay element but must still collide as a full cube).
        let is_solid = refs.iter().any(|r| self.model_has_full_cube(&r.model));

        // Fast path: a single, unrotated, full-cube model.
        if refs.len() == 1 && refs[0].x == 0 && refs[0].y == 0 {
            if let Some(cube) = self.try_cube(&refs[0].model, foliage) {
                return Some((cube, true));
            }
        }

        let mut quads = Vec::new();
        for r in &refs {
            self.append_model_quads(r, foliage, &mut quads);
        }
        if quads.is_empty() {
            return None;
        }
        let idx = models.len() as u32;
        models.push(quads);
        Some((RenderKind::Model(idx), is_solid))
    }

    /// True if the model (or a parent) has any element spanning the full
    /// 16³ box — the collision-cube test.
    fn model_has_full_cube(&mut self, model: &str) -> bool {
        let Some(resolved) = self.resolve_model(model) else {
            return false;
        };
        resolved.elements.iter().any(|el| {
            box_coords(el, "from") == Some([0.0, 0.0, 0.0])
                && box_coords(el, "to") == Some([16.0, 16.0, 16.0])
        })
    }

    /// If `model` is a full 16³ cube with all six faces, return a Cube.
    fn try_cube(&mut self, model: &str, foliage: bool) -> Option<RenderKind> {
        let resolved = self.resolve_model(model)?;
        if resolved.elements.len() != 1 {
            return None;
        }
        let el = &resolved.elements[0];
        if box_coords(el, "from")? != [0.0, 0.0, 0.0]
            || box_coords(el, "to")? != [16.0, 16.0, 16.0]
        {
            return None;
        }
        if el.get("rotation").is_some() {
            return None;
        }
        let faces = el.get("faces")?.as_object()?;
        let mut layers = [0u16; 6];
        let mut tint = [false; 6];
        for (i, name) in FACE_NAMES.iter().enumerate() {
            let face = faces.get(*name)?;
            let tex = face.get("texture")?.as_str()?;
            let tex_name = resolve_texture_var(tex, &resolved.textures)?;
            layers[i] = self.layer_for(&tex_name, foliage_of(&tex_name, foliage))?;
            tint[i] = face.get("tintindex").is_some();
        }
        Some(RenderKind::Cube {
            faces: layers,
            tint,
        })
    }

    fn append_model_quads(&mut self, r: &ModelRef, foliage: bool, out: &mut Vec<Quad>) {
        let Some(resolved) = self.resolve_model(&r.model) else {
            return;
        };
        let shade_default = resolved.ambient_occlusion; // proxy; real shade is per-element
        for el in &resolved.elements {
            self.element_quads(el, &resolved.textures, r.x, r.y, foliage, shade_default, out);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn element_quads(
        &mut self,
        el: &serde_json::Value,
        textures: &HashMap<String, String>,
        rot_x: i32,
        rot_y: i32,
        foliage: bool,
        _shade_default: bool,
        out: &mut Vec<Quad>,
    ) {
        let (Some(from), Some(to)) = (box_coords(el, "from"), box_coords(el, "to")) else {
            return;
        };
        let shade = el.get("shade").and_then(|s| s.as_bool()).unwrap_or(true);
        let el_rot = el.get("rotation");
        let Some(faces) = el.get("faces").and_then(|f| f.as_object()) else {
            return;
        };
        for (fname, face) in faces {
            let Some(face_idx) = FACE_NAMES.iter().position(|n| n == fname) else {
                continue;
            };
            let Some(tex) = face.get("texture").and_then(|t| t.as_str()) else {
                continue;
            };
            let Some(tex_name) = resolve_texture_var(tex, textures) else {
                continue;
            };
            let Some(layer) = self.layer_for(&tex_name, foliage_of(&tex_name, foliage)) else {
                continue;
            };
            let tint = face.get("tintindex").is_some();
            let has_cull = face.get("cullface").is_some();

            // Corners in 0..16, then transforms, then /16.
            let (mut verts, uv) = face_geometry(from, to, face_idx, face.get("uv"));
            // Element rotation.
            if let Some(rot) = el_rot {
                apply_element_rotation(&mut verts, rot);
            }
            // Whole-model variant rotation (x then y) around (8,8,8).
            for v in verts.iter_mut() {
                rotate_xy(v, rot_x, rot_y);
            }
            let mut fverts = [[0.0f32; 3]; 4];
            for (i, v) in verts.iter().enumerate() {
                fverts[i] = [(v[0] / 16.0), (v[1] / 16.0), (v[2] / 16.0)];
            }

            // Rotated normal → shade dir + (if declared) cullface dir.
            let mut normal = FACE_NORMALS[face_idx];
            if let Some(rot) = el_rot {
                rotate_normal_element(&mut normal, rot);
            }
            rotate_normal_xy(&mut normal, rot_x, rot_y);
            let dir = snap_face(normal);
            let cull = if has_cull && is_axis_flush(&fverts, dir) {
                dir as i8
            } else {
                -1
            };

            out.push(Quad {
                verts: fverts,
                uv,
                layer,
                cull,
                dir,
                tint,
                shade,
            });
        }
    }

    fn resolve_model(&mut self, model: &str) -> Option<ResolvedModel> {
        let key = model.strip_prefix("minecraft:").unwrap_or(model).to_string();
        if let Some(c) = self.model_cache.get(&key) {
            return c.clone();
        }
        let resolved = self.resolve_model_uncached(&key);
        self.model_cache.insert(key, resolved.clone());
        resolved
    }

    fn resolve_model_uncached(&mut self, key: &str) -> Option<ResolvedModel> {
        let json = self.read_json(&format!("assets/minecraft/models/{key}.json"))?;
        let mut out = match json.get("parent").and_then(|p| p.as_str()) {
            Some(parent) => {
                // "builtin/*" parents (entity models) have no geometry.
                if parent.contains("builtin/") {
                    ResolvedModel {
                        textures: HashMap::new(),
                        elements: Vec::new(),
                        ambient_occlusion: true,
                    }
                } else {
                    self.resolve_model(parent).unwrap_or(ResolvedModel {
                        textures: HashMap::new(),
                        elements: Vec::new(),
                        ambient_occlusion: true,
                    })
                }
            }
            None => ResolvedModel {
                textures: HashMap::new(),
                elements: Vec::new(),
                ambient_occlusion: true,
            },
        };
        if let Some(textures) = json.get("textures").and_then(|t| t.as_object()) {
            for (var, value) in textures {
                // 26.x allows a texture to be either a plain ref string OR
                // an object `{sprite, force_translucent, …}` carrying render
                // metadata — take the `sprite` in the object case (glass,
                // tinted glass, ice, …), else the string.
                let name = value.as_str().map(str::to_string).or_else(|| {
                    value
                        .get("sprite")
                        .and_then(|s| s.as_str())
                        .map(str::to_string)
                });
                if let Some(name) = name {
                    out.textures.insert(var.clone(), name);
                }
            }
        }
        // A model defining its own elements overrides the parent's.
        if let Some(elements) = json.get("elements").and_then(|e| e.as_array()) {
            out.elements = elements.clone();
        }
        if let Some(ao) = json.get("ambientocclusion").and_then(|a| a.as_bool()) {
            out.ambient_occlusion = ao;
        }
        Some(out)
    }

    /// Texture-array layer for a texture name; `foliage` picks the tint
    /// colormap for grayscale grass/foliage textures.
    fn layer_for(&mut self, tex_name: &str, apply_tint: TintKind) -> Option<u16> {
        let cache_key = format!("{tex_name}#{apply_tint:?}");
        if let Some(&layer) = self.layer_index.get(&cache_key) {
            return Some(layer);
        }
        let short = tex_name.strip_prefix("minecraft:").unwrap_or(tex_name);
        let path = format!("assets/minecraft/textures/{short}.png");
        let mut bytes = Vec::new();
        self.jar.by_name(&path).ok()?.read_to_end(&mut bytes).ok()?;
        let mut rgba = decode_png_rgba(&bytes)?;
        // The `grass_block_top` / `short_grass` textures ship grayscale and
        // expect a colormap multiply — bake the plains color in so their
        // layer reads green. Per-biome variation is deferred.
        match apply_tint {
            TintKind::Grass => tint_rgb(&mut rgba, self.grass_tint),
            TintKind::Foliage => tint_rgb(&mut rgba, self.foliage_tint),
            // Water ships grayscale + alpha 180; biome water color is a
            // registry value, not a colormap — plains #3F76E4 baked (the
            // same fixed-plains approach as grass until per-biome tint).
            TintKind::Water => tint_rgb(&mut rgba, [0x3F, 0x76, 0xE4]),
            TintKind::None => {}
        }
        let layer = self.layers.len() as u16;
        self.layers.push(rgba);
        self.layer_names.push(cache_key.clone());
        self.layer_index.insert(cache_key, layer);
        Some(layer)
    }
}

#[derive(Clone, Copy, Debug)]
enum TintKind {
    None,
    Water,
    Grass,
    Foliage,
}

/// Grayscale textures that must be colormap-tinted at bake time (their model
/// faces don't always carry a tintindex, e.g. grass_block_top).
fn foliage_of(tex_name: &str, block_is_foliage: bool) -> TintKind {
    let short = tex_name.rsplit('/').next().unwrap_or(tex_name);
    if short.contains("grass_block_top") || short == "short_grass" || short == "tall_grass_top" {
        TintKind::Grass
    } else if block_is_foliage {
        TintKind::Foliage
    } else {
        TintKind::None
    }
}

fn is_foliage(block: &str) -> bool {
    block.ends_with("_leaves") || block == "vine" || block.contains("_stem")
}

// -- geometry ----------------------------------------------------------------

/// Corner positions (0..16) + uv (0..1) for one face of a box, textures
/// upright. Winding is irrelevant (mesher renders cull-none); only the
/// corner↔uv pairing matters visually.
fn face_geometry(
    f: [f32; 3],
    t: [f32; 3],
    face: usize,
    uv_field: Option<&serde_json::Value>,
) -> ([[f32; 3]; 4], [[f32; 2]; 4]) {
    let (fx, fy, fz) = (f[0], f[1], f[2]);
    let (tx, ty, tz) = (t[0], t[1], t[2]);
    // [TL, TR, BR, BL] with textures upright (+Y is up for side faces).
    let verts = match face {
        0 => [
            [fx, ty, fz],
            [tx, ty, fz],
            [tx, ty, tz],
            [fx, ty, tz],
        ], // up
        1 => [
            [fx, fy, fz],
            [tx, fy, fz],
            [tx, fy, tz],
            [fx, fy, tz],
        ], // down
        2 => [
            [tx, ty, fz],
            [fx, ty, fz],
            [fx, fy, fz],
            [tx, fy, fz],
        ], // north (-Z)
        3 => [
            [fx, ty, tz],
            [tx, ty, tz],
            [tx, fy, tz],
            [fx, fy, tz],
        ], // south (+Z)
        4 => [
            [fx, ty, fz],
            [fx, ty, tz],
            [fx, fy, tz],
            [fx, fy, fz],
        ], // west (-X)
        _ => [
            [tx, ty, tz],
            [tx, ty, fz],
            [tx, fy, fz],
            [tx, fy, tz],
        ], // east (+X)
    };
    // uv rect [u1,v1,u2,v2] in 0..16 texture space; default = face extent.
    let rect = uv_field
        .and_then(|u| u.as_array())
        .filter(|a| a.len() == 4)
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(0.0) as f32,
                a[1].as_f64().unwrap_or(0.0) as f32,
                a[2].as_f64().unwrap_or(16.0) as f32,
                a[3].as_f64().unwrap_or(16.0) as f32,
            ]
        })
        .unwrap_or_else(|| default_uv(f, t, face));
    let (u1, v1, u2, v2) = (rect[0] / 16.0, rect[1] / 16.0, rect[2] / 16.0, rect[3] / 16.0);
    let uv = [[u1, v1], [u2, v1], [u2, v2], [u1, v2]];
    (verts, uv)
}

/// Default uv from the element's extent projected onto the face.
fn default_uv(f: [f32; 3], t: [f32; 3], face: usize) -> [f32; 4] {
    match face {
        0 | 1 => [f[0], f[2], t[0], t[2]], // x,z
        2 | 3 => [f[0], 16.0 - t[1], t[0], 16.0 - f[1]], // x,y
        _ => [f[2], 16.0 - t[1], t[2], 16.0 - f[1]],     // z,y
    }
}

fn box_coords(el: &serde_json::Value, key: &str) -> Option<[f32; 3]> {
    let a = el.get(key)?.as_array()?;
    Some([
        a[0].as_f64()? as f32,
        a[1].as_f64()? as f32,
        a[2].as_f64()? as f32,
    ])
}

/// Rotate corners around an element rotation {origin, axis, angle, rescale}.
fn apply_element_rotation(verts: &mut [[f32; 3]; 4], rot: &serde_json::Value) {
    let origin = rot
        .get("origin")
        .and_then(|o| o.as_array())
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(8.0) as f32,
                a[1].as_f64().unwrap_or(8.0) as f32,
                a[2].as_f64().unwrap_or(8.0) as f32,
            ]
        })
        .unwrap_or([8.0, 8.0, 8.0]);
    let axis = rot.get("axis").and_then(|a| a.as_str()).unwrap_or("y");
    let angle = rot.get("angle").and_then(|a| a.as_f64()).unwrap_or(0.0) as f32;
    let rescale = rot.get("rescale").and_then(|r| r.as_bool()).unwrap_or(false);
    let rad = angle.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let scale = if rescale && c.abs() > 1e-4 {
        1.0 / c.abs()
    } else {
        1.0
    };
    let ai = match axis {
        "x" => 0,
        "z" => 2,
        _ => 1,
    };
    for v in verts.iter_mut() {
        let mut p = [v[0] - origin[0], v[1] - origin[1], v[2] - origin[2]];
        let (a, b) = match ai {
            0 => (1, 2),
            2 => (0, 1),
            _ => (2, 0),
        };
        let (pa, pb) = (p[a], p[b]);
        p[a] = pa * c - pb * s;
        p[b] = pa * s + pb * c;
        if rescale {
            p[a] *= scale;
            p[b] *= scale;
        }
        v[0] = p[0] + origin[0];
        v[1] = p[1] + origin[1];
        v[2] = p[2] + origin[2];
    }
}

/// Rotate a point (0..16) by whole-model x then y (0/90/180/270) around
/// block center (8,8,8).
fn rotate_xy(v: &mut [f32; 3], x: i32, y: i32) {
    let c = [8.0f32, 8.0, 8.0];
    let mut p = [v[0] - c[0], v[1] - c[1], v[2] - c[2]];
    for _ in 0..((x / 90).rem_euclid(4)) {
        // x rotation: y→z, z→-y  (90° about +X)
        let (ny, nz) = (-p[2], p[1]);
        p[1] = ny;
        p[2] = nz;
    }
    for _ in 0..((y / 90).rem_euclid(4)) {
        // y rotation: x→-z, z→x  (90° about +Y)
        let (nx, nz) = (p[2], -p[0]);
        p[0] = nx;
        p[2] = nz;
    }
    v[0] = p[0] + c[0];
    v[1] = p[1] + c[1];
    v[2] = p[2] + c[2];
}

fn rotate_normal_element(n: &mut [f32; 3], rot: &serde_json::Value) {
    let axis = rot.get("axis").and_then(|a| a.as_str()).unwrap_or("y");
    let angle = rot.get("angle").and_then(|a| a.as_f64()).unwrap_or(0.0) as f32;
    let rad = angle.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (a, b) = match axis {
        "x" => (1, 2),
        "z" => (0, 1),
        _ => (2, 0),
    };
    let (na, nb) = (n[a], n[b]);
    n[a] = na * c - nb * s;
    n[b] = na * s + nb * c;
}

fn rotate_normal_xy(n: &mut [f32; 3], x: i32, y: i32) {
    for _ in 0..((x / 90).rem_euclid(4)) {
        let (ny, nz) = (-n[2], n[1]);
        n[1] = ny;
        n[2] = nz;
    }
    for _ in 0..((y / 90).rem_euclid(4)) {
        let (nx, nz) = (n[2], -n[0]);
        n[0] = nx;
        n[2] = nz;
    }
}

/// Snap a normal to the nearest of the six face directions.
fn snap_face(n: [f32; 3]) -> u8 {
    let mut best = 0u8;
    let mut best_dot = f32::MIN;
    for (i, fn_) in FACE_NORMALS.iter().enumerate() {
        let d = n[0] * fn_[0] + n[1] * fn_[1] + n[2] * fn_[2];
        if d > best_dot {
            best_dot = d;
            best = i as u8;
        }
    }
    best
}

/// True if all 4 corners sit flush against the block boundary on `dir`'s axis
/// (so a cullface is meaningful after rotation).
fn is_axis_flush(verts: &[[f32; 3]; 4], dir: u8) -> bool {
    let (axis, val) = match dir {
        0 => (1, 1.0),
        1 => (1, 0.0),
        2 => (2, 0.0),
        3 => (2, 1.0),
        4 => (0, 0.0),
        _ => (0, 1.0),
    };
    verts.iter().all(|v| (v[axis] - val).abs() < 1e-3)
}

// -- variant / multipart selection -------------------------------------------

fn model_ref(apply: &serde_json::Value) -> Option<ModelRef> {
    // `apply` may be an object or an array (random weighted) — take the first.
    let obj = if let Some(arr) = apply.as_array() {
        arr.first()?
    } else {
        apply
    };
    Some(ModelRef {
        model: obj.get("model")?.as_str()?.to_string(),
        x: obj.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        y: obj.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
    })
}

fn pick_variant(
    variants: &serde_json::Map<String, serde_json::Value>,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<ModelRef> {
    for (key, value) in variants {
        if variant_matches(key, props) {
            return model_ref(value);
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
    let Some(props) = props else { return false };
    key.split(',').all(|pair| {
        let Some((k, v)) = pair.split_once('=') else {
            return false;
        };
        props.get(k).and_then(|pv| pv.as_str()) == Some(v)
    })
}

/// Evaluate a multipart entry's `when` against the state properties.
fn multipart_applies(
    part: &serde_json::Value,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    let Some(when) = part.get("when") else {
        return true; // unconditional
    };
    when_matches(when, props)
}

fn when_matches(
    when: &serde_json::Value,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    // OR of sub-conditions.
    if let Some(or) = when.get("OR").and_then(|o| o.as_array()) {
        return or.iter().any(|c| when_matches(c, props));
    }
    if let Some(and) = when.get("AND").and_then(|a| a.as_array()) {
        return and.iter().all(|c| when_matches(c, props));
    }
    let Some(obj) = when.as_object() else {
        return true;
    };
    let Some(props) = props else { return false };
    obj.iter().all(|(k, v)| {
        let Some(pv) = props.get(k).and_then(|p| p.as_str()) else {
            return false;
        };
        // Value may be "a|b" (any-of).
        match v.as_str() {
            Some(s) => s.split('|').any(|opt| opt == pv),
            None => false,
        }
    })
}

// -- textures ----------------------------------------------------------------

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

fn decode_png_rgba(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
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
        png::ColorType::Rgba => rgba.copy_from_slice(&buf[..px * px * 4]),
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
        png::ColorType::Indexed => return None,
    }
    Some(rgba)
}

/// Center pixel of a colormap PNG (the "plains" reference point).
fn colormap_center(jar: Jar, name: &str) -> Option<[u8; 3]> {
    let path = format!("assets/minecraft/textures/colormap/{name}.png");
    let mut bytes = Vec::new();
    jar.by_name(&path).ok()?.read_to_end(&mut bytes).ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return None,
    };
    // Plains ≈ (temp 0.8, downpour 0.4) → about 62% across, 45% down.
    let x = (w as f32 * 0.55) as usize;
    let y = (h as f32 * 0.55) as usize;
    let i = (y.min(h - 1) * w + x.min(w - 1)) * ch;
    Some([buf[i], buf[i + 1], buf[i + 2]])
}

fn tint_rgb(rgba: &mut [u8], color: [u8; 3]) {
    for px in rgba.chunks_exact_mut(4) {
        for c in 0..3 {
            px[c] = ((px[c] as u16 * color[c] as u16) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_face_picks_axis() {
        assert_eq!(snap_face([0.0, 1.0, 0.0]), 0); // up
        assert_eq!(snap_face([0.0, -0.9, 0.1]), 1); // down
        assert_eq!(snap_face([0.0, 0.0, -1.0]), 2); // north
        assert_eq!(snap_face([1.0, 0.0, 0.0]), 5); // east
    }

    #[test]
    fn y_rotation_cycles_north_to_east() {
        // north normal (0,0,-1) rotated y=90 → east/west axis.
        let mut n = [0.0, 0.0, -1.0];
        rotate_normal_xy(&mut n, 0, 90);
        assert_eq!(snap_face(n), 4); // -X (west) — a 90° turn off north
    }

    #[test]
    fn slab_default_uv_covers_face() {
        // Full 16-wide top face → uv spans 0..1.
        let uv = default_uv([0.0, 0.0, 0.0], [16.0, 8.0, 16.0], 0);
        assert_eq!(uv, [0.0, 0.0, 16.0, 16.0]);
    }

    #[test]
    fn font_advance_measures_rightmost_lit_column() {
        // 16×16 grid of 2-px cells; light up column 1 of glyph 'A' (65).
        let (size, cell) = (32u32, 2u32);
        let mut atlas = vec![0u8; (size * size * 4) as usize];
        let (cx, cy) = ((65 % 16) * cell, (65 / 16) * cell);
        atlas[((cy * size + cx + 1) * 4 + 3) as usize] = 255;
        let adv = font_advances(&atlas, size, cell);
        assert_eq!(adv[65], 3, "rightmost col 1 → advance 1+2");
        assert_eq!(adv[32], 4, "blank space cell advances 4");
    }
}
