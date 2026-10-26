//! Baked held-item models (M22) — the unified output of both geometry paths.
//!
//! An item reaches the screen one of two ways, and they converge here:
//!
//! - a `minecraft:block/…` reference reuses the block model the asset bake
//!   already resolves, whose quads carry a **texture-array layer index**; the
//!   layer's pixels are copied out so the entity pass can sample them from its
//!   own atlas, which is the whole reason this type exists rather than the
//!   renderer consuming [`crate::assets::Quad`] directly;
//! - a `builtin/generated` chain extrudes the item sprite
//!   ([`crate::item_geometry`]), whose texture is the sprite PNG.
//!
//! Both end as quads in **model units 0..16** with UVs in **0..1 of their own
//! texture**, plus a list of textures to pack. The renderer therefore has one
//! code path and never needs to know which source an item came from.

use std::collections::{BTreeMap, HashMap};

use crate::item_models::DisplayTransform;

/// One texture an item's quads sample, as decoded RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldTexture {
    pub w: u32,
    pub h: u32,
    /// `w * h * 4` bytes.
    pub rgba: Vec<u8>,
}

/// M272/M275 — what [`HeldItems::add_skin_head_model`] made.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinHeadModels {
    /// The placed (and worn) head's block-entity model.
    pub block: String,
    /// The new texture's index.
    pub tex: usize,
    /// The item's model (`None` when the bake has no player-head item).
    pub item: Option<String>,
}

/// One quad of a held item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldQuad {
    /// Corners in model units `0..16`.
    pub verts: [[f32; 3]; 4],
    /// UVs in `0..1` of [`Self::tex`].
    pub uv: [[f32; 2]; 4],
    /// Index into [`HeldItems::textures`].
    pub tex: u16,
    /// Which animated group this quad belongs to.
    ///
    /// `0` is static, and every item quad is static — an item is placed by one
    /// display transform and never articulates. Block-entity models use it to
    /// name the parts their renderer animates: a chest's `1` is the lid *and*
    /// its lock, because `ChestModel.setupAnim` gives both the same `xRot`
    /// about the same pivot.
    pub part: u8,
    /// Vanilla `Direction` ordinal (down 0, up 1, north 2, south 3, west 4,
    /// east 5) for directional shading.
    pub dir: u8,
}

/// A baked held item.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldItemModel {
    pub quads: Vec<HeldQuad>,
    /// `display.thirdperson_righthand`.
    pub right: DisplayTransform,
    /// `display.thirdperson_lefthand`.
    pub left: DisplayTransform,
    /// `display.ground` — the context a *dropped* stack renders through
    /// (`ItemEntityRenderer` → `updateForNonLiving(..., GROUND, ...)`).
    pub ground: DisplayTransform,
    /// `display.gui` — how the item sits in a hotbar or inventory slot (M34).
    /// Identity for every extruded sprite, which is exactly right for them: a
    /// flat quad facing the viewer, filling the slot. Blocks inherit
    /// `block/block.json`'s `rotation [30, 225, 0], scale 0.625`.
    pub gui: DisplayTransform,
    /// `display.firstperson_righthand` — the pose the held item takes in the
    /// player's own view (M38).
    pub first_right: DisplayTransform,
    /// `display.firstperson_lefthand`, with vanilla's absent-left fallback to
    /// the right-hand entry already applied. The left/right **mirror** is not
    /// baked in: `ItemTransform.apply` negates `translation.x`, `rotation.y`
    /// and `rotation.z` at draw time for either hand's transform whenever the
    /// context is a left hand.
    pub first_left: DisplayTransform,
    /// `display.fixed` — the context an item frame draws its item through
    /// (M194; `ItemFrameRenderer` → `updateForNonLiving(..., FIXED, ...)`).
    pub fixed: DisplayTransform,
    /// `display.head` — the context `CustomHeadLayer` draws a worn non-skull
    /// item through (M274).
    pub head: DisplayTransform,
    /// True when the geometry came from a block model — recorded so the
    /// renderer and the gate can tell the two sources apart without
    /// re-deriving it.
    pub from_block: bool,
    /// Separate geometry for the `gui` context, when the item's definition
    /// actually selects a different model there (M40).
    ///
    /// `None` for all but a handful: only a `minecraft:select` on
    /// `minecraft:display_context` can produce this, and in 26.2 it is the
    /// spears and the bundles. A spear is a flat sprite in a slot and a real
    /// 3D model in the hand, so one geometry cannot serve both — this is not
    /// a transform difference, which the six `DisplayTransform`s above already
    /// cover, but different quads.
    pub gui_quads: Option<Vec<HeldQuad>>,
}

/// Every resolvable item's baked model, keyed by full registry name.
#[derive(Clone, Debug, Default)]
pub struct HeldItems {
    pub models: HashMap<String, HeldItemModel>,
    /// Block-entity models (M25b), in the same shape and sharing the same
    /// texture pool, but **kept in their own map**: they are not items, and
    /// folding them in inflated the item counts the `itemshot` gate asserts
    /// against the jar's registry — which is exactly how this was caught.
    pub block_entities: HashMap<String, HeldItemModel>,
    pub textures: Vec<HeldTexture>,
    /// Definition types that were deliberately not resolved, and how many
    /// items each accounted for. Sorted so the load log is stable.
    pub unsupported: BTreeMap<String, usize>,
    /// M234 — the definitions (their `model` trees) of the items whose model
    /// branches on the owner's use or a crossbow's charge, keyed by full
    /// name; their variants are baked into [`Self::models`] as
    /// `"<item>#<model reference>"`.
    pub dynamic: HashMap<String, serde_json::Value>,
    /// M405 — the banner and shield pattern sheets by texture path
    /// (`entity/banner/<stem>`, `entity/shield/<stem>`, and the shield's
    /// `base` and `shield_base`). Kept on the CPU only: they are composited
    /// into per-stack copies ([`Self::add_pattern_model`]), never drawn.
    pub pattern_sheets: HashMap<String, HeldTexture>,
    /// M405 — each item's dye-tinted part textures (a banner's flag, M244),
    /// the sheets a pattern copy composites over.
    pub tinted_parts: HashMap<String, Vec<u16>>,
}

/// M405 — a banner or shield item copied for one stack's patterns:
/// its model name and the textures appended for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternModel {
    pub name: String,
    pub textures: Vec<usize>,
}

/// M405 — `BannerRenderer.submitPatterns` composited into one sheet: each
/// layer's mask tinted by its dye (`0xRRGGBB`, multiplied into the 8-bit
/// sRGB texel as vanilla's vertex colour is) and alpha-blended over `base`
/// (`SRC_ALPHA, ONE_MINUS_SRC_ALPHA`), in the 8-bit gamma space vanilla's
/// framebuffer blends in. The layers are coplanar and share one light, so
/// blending first and lighting after gives vanilla's pixels. `None` when a
/// layer's size differs from the base's.
pub fn composite_layers(base: &HeldTexture, layers: &[(&HeldTexture, u32)]) -> Option<HeldTexture> {
    let mut out = base.clone();
    for (mask, rgb) in layers {
        if (mask.w, mask.h) != (base.w, base.h) {
            return None;
        }
        let tint = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF];
        for (d, s) in out
            .rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(mask.rgba.as_chunks::<4>().0)
        {
            let a = s[3] as u32;
            if a == 0 {
                continue;
            }
            for c in 0..3 {
                let src = (s[c] as u32 * tint[c] + 127) / 255;
                d[c] = ((src * a + d[c] as u32 * (255 - a) + 127) / 255) as u8;
            }
            d[3] = (a + (d[3] as u32 * (255 - a) + 127) / 255).min(255) as u8;
        }
    }
    Some(out)
}

/// M405 — FNV-1a over a pattern list, for a stable variant name.
fn pattern_key(base: Option<u8>, layers: &[(String, u8)]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(base.unwrap_or(0xFF));
    for (stem, dye) in layers {
        for b in stem.bytes() {
            eat(b);
        }
        eat(0);
        eat(*dye);
    }
    h
}

/// M234 — what [`UseVariants::select`] needs: the definitions of the items
/// whose model branches on use, and which `"<item>#<model reference>"`
/// variants of them baked.
#[derive(Clone, Debug, Default)]
pub struct UseVariants {
    dynamic: HashMap<String, serde_json::Value>,
    keys: std::collections::HashSet<String>,
}

impl UseVariants {
    /// The model to draw for `name` in `ctx`: the variant the item's
    /// definition reduces to with the live state (`IsUsingItem`,
    /// `UseDuration`, `CrossbowPull`, `Charge`), or `name` itself for an item
    /// whose model never branches on use, a reduction that lands on the
    /// plain model, or a variant that did not bake.
    pub fn select<'s>(
        &'s self,
        name: &'s str,
        ctx: crate::item_models::SelectionContext<'_>,
    ) -> &'s str {
        let Some(root) = self.dynamic.get(name) else {
            return name;
        };
        let Some(reference) = crate::item_models::reduce_definition(root, ctx)
            .ok()
            .and_then(|leaf| leaf.get("model"))
            .and_then(|m| m.as_str())
        else {
            return name;
        };
        self.keys
            .get(&format!("{name}#{reference}"))
            .map_or(name, String::as_str)
    }

    /// Whether `name`'s model branches on use at all.
    pub fn is_dynamic(&self, name: &str) -> bool {
        self.dynamic.contains_key(name)
    }

    /// How many variants baked (for the gate).
    pub fn variant_count(&self) -> usize {
        self.keys.len()
    }
}

impl HeldItemModel {
    /// The quads to draw in a given context — [`Self::gui_quads`] for a slot
    /// when the item has them, [`Self::quads`] otherwise.
    pub fn quads_for_gui(&self) -> &[HeldQuad] {
        self.gui_quads.as_deref().unwrap_or(&self.quads)
    }
}

impl HeldItems {
    pub fn get(&self, name: &str) -> Option<&HeldItemModel> {
        self.models.get(name)
    }

    /// M272 — a player head wearing a skin of its own: the 64×64 `rgba`
    /// appended to the textures and a copy of `rewo:be/player_head` (head
    /// and hat, the full sheet) sampling it, named
    /// `rewo:be/player_head#<texture index>`. Returns the name and the new
    /// texture's index, or `None` for a buffer of the wrong size or no
    /// player-head model.
    ///
    /// M275 — and the item's: a copy of `minecraft:player_head` (the head
    /// model under the item's display transforms) with every quad that
    /// sampled the default sheet sampling the new one, named
    /// `minecraft:player_head#skin<texture index>` and returned third.
    pub fn add_skin_head_model(&mut self, rgba: Vec<u8>) -> Option<SkinHeadModels> {
        if rgba.len() != 64 * 64 * 4 {
            return None;
        }
        let mut model = self.any("rewo:be/player_head")?.clone();
        let default_tex = model.quads.first()?.tex;
        let tex = self.textures.len();
        let t = u16::try_from(tex).ok()?;
        self.textures.push(HeldTexture { w: 64, h: 64, rgba });
        for q in &mut model.quads {
            q.tex = t;
        }
        let block = format!("rewo:be/player_head#{tex}");
        self.block_entities.insert(block.clone(), model);
        let item = self
            .models
            .get("minecraft:player_head")
            .cloned()
            .map(|mut m| {
                for q in &mut m.quads {
                    if q.tex == default_tex {
                        q.tex = t;
                    }
                }
                let name = format!("minecraft:player_head#skin{tex}");
                self.models.insert(name.clone(), m);
                name
            });
        Some(SkinHeadModels { block, tex, item })
    }

    /// M405 — `item` drawn with one stack's banner patterns: a copy of its
    /// model whose patterned sheet is the composite of
    /// `BannerRenderer.submitPatterns` (at most 16 layers, as vanilla's loop
    /// stops), named `"<item>#pattern<hash>"` and built once per pattern
    /// list.
    ///
    /// A banner's flag already samples its dye-tinted base (M244), so its
    /// patterns go over that. A shield's whole model samples one sheet:
    /// `shield_base` when it has patterns or a base colour, then the
    /// tinted `shield/base` (`base_color`, white when absent), then the
    /// patterns from `entity/shield/`. `layers` are `(stem, DyeColor id)`;
    /// a stem with no sheet is skipped. `None` for an item that is neither,
    /// or a shield with nothing to draw (its plain model is right).
    pub fn add_pattern_model(
        &mut self,
        item: &str,
        base: Option<u8>,
        layers: &[(String, u8)],
    ) -> Option<PatternModel> {
        let dye = |i: u8| {
            crate::block_entity_models::DYE_DIFFUSE_COLORS
                .get(i as usize)
                .copied()
                .unwrap_or(0xFFFFFF)
        };
        let layers = &layers[..layers.len().min(16)];
        let name = format!("{item}#pattern{:016x}", pattern_key(base, layers));
        if self.models.contains_key(&name) {
            return Some(PatternModel {
                name,
                textures: Vec::new(),
            });
        }
        let mut model = self.models.get(item)?.clone();
        let shield = item == "minecraft:shield";
        // (old texture, new sheet) for each sheet the copy replaces.
        let mut swaps: Vec<(u16, HeldTexture)> = Vec::new();
        if shield {
            if layers.is_empty() && base.is_none() {
                return None;
            }
            let old = model.quads.first()?.tex;
            let sheet = |p: &str| self.pattern_sheets.get(p);
            let mut ls: Vec<(&HeldTexture, u32)> =
                vec![(sheet("entity/shield/base")?, dye(base.unwrap_or(0)))];
            for (stem, d) in layers {
                if let Some(m) = sheet(&format!("entity/shield/{stem}")) {
                    ls.push((m, dye(*d)));
                }
            }
            swaps.push((
                old,
                composite_layers(sheet("entity/shield/shield_base")?, &ls)?,
            ));
        } else {
            let flags = self.tinted_parts.get(item)?;
            if layers.is_empty() {
                return None;
            }
            for &old in flags {
                let ls: Vec<(&HeldTexture, u32)> = layers
                    .iter()
                    .filter_map(|(stem, d)| {
                        self.pattern_sheets
                            .get(&format!("entity/banner/{stem}"))
                            .map(|m| (m, dye(*d)))
                    })
                    .collect();
                swaps.push((
                    old,
                    composite_layers(self.textures.get(old as usize)?, &ls)?,
                ));
            }
        }
        let mut textures = Vec::new();
        for (old, sheet) in swaps {
            let new = u16::try_from(self.textures.len()).ok()?;
            self.textures.push(sheet);
            textures.push(new as usize);
            for q in model
                .quads
                .iter_mut()
                .chain(model.gui_quads.iter_mut().flatten())
            {
                if q.tex == old {
                    q.tex = new;
                }
            }
        }
        self.models.insert(name.clone(), model);
        Some(PatternModel { name, textures })
    }

    /// M407 — `minecraft:decorated_pot` drawn with one stack's sherds: a
    /// copy whose four side planes (the item's plain-side quads, in
    /// `DecoratedPotRenderer`'s back, left, right, front order, the order the
    /// item's parts were assembled in) sample each sherd's pattern sheet —
    /// the placed pot's `rewo:be/pot_side/<stem>` texture. `sherds` are item
    /// names, `None` (or brick, or anything not a sherd) the plain side.
    /// Named `"minecraft:decorated_pot#sherds<hash>"`; `None` when every side
    /// is plain or the item has no plain sides to repoint.
    pub fn add_pot_model(&mut self, sherds: [Option<&str>; 4]) -> Option<PatternModel> {
        use crate::block_entity_models::{pot_side_model, POT_SIDE_PLAIN};
        let plain_model = POT_SIDE_PLAIN.0;
        let side_tex = |name: &str| {
            self.block_entities
                .get(name)
                .and_then(|m| m.quads.first())
                .map(|q| q.tex)
        };
        let plain = side_tex(plain_model)?;
        let texs: Vec<u16> = sherds
            .iter()
            .map(|s| side_tex(&pot_side_model(*s)).unwrap_or(plain))
            .collect();
        if texs.iter().all(|&t| t == plain) {
            return None;
        }
        let mut key = 0xcbf2_9ce4_8422_2325u64;
        for t in &texs {
            for b in t.to_le_bytes() {
                key ^= b as u64;
                key = key.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        let name = format!("minecraft:decorated_pot#sherds{key:016x}");
        if !self.models.contains_key(&name) {
            let mut model = self.models.get("minecraft:decorated_pot")?.clone();
            let mut k = 0;
            for q in model.quads.iter_mut().filter(|q| q.tex == plain) {
                if let Some(&t) = texs.get(k) {
                    q.tex = t;
                }
                k += 1;
            }
            if k != 4 {
                return None;
            }
            self.models.insert(name.clone(), model);
        }
        // The sheets are the placed pot's: nothing new to upload.
        Some(PatternModel {
            name,
            textures: Vec::new(),
        })
    }

    /// Look a model up by name across both maps — what the renderer wants,
    /// since a draw carries a name and does not care which kind it is. The
    /// namespaces cannot collide: block-entity models are `rewo:be/…`.
    pub fn any(&self, name: &str) -> Option<&HeldItemModel> {
        if let Some(m) = self
            .models
            .get(name)
            .or_else(|| self.block_entities.get(name))
        {
            return Some(m);
        }
        // M49: `"<item>#<trim material>"` is a trimmed icon variant. Falling
        // back to the plain item is not a nicety — an item can be trimmed with
        // a material its own definition names no case for, and vanilla's
        // answer there is the `fallback`, which *is* the untrimmed icon. So a
        // missing variant must degrade to it rather than to no icon at all.
        let (base, _) = name.split_once('#')?;
        self.models.get(base)
    }

    /// M234 — the use-variant table the draw path selects with: the dynamic
    /// definitions and the variant keys that baked, without the geometry.
    pub fn use_variants(&self) -> UseVariants {
        UseVariants {
            dynamic: self.dynamic.clone(),
            keys: self
                .models
                .keys()
                .filter(|k| {
                    k.split_once('#')
                        .is_some_and(|(base, _)| self.dynamic.contains_key(base))
                })
                .cloned()
                .collect(),
        }
    }

    /// Items whose geometry came from a block model.
    pub fn block_count(&self) -> usize {
        self.models.values().filter(|m| m.from_block).count()
    }

    /// Items whose geometry came from an extruded sprite.
    pub fn sprite_count(&self) -> usize {
        self.models.values().filter(|m| !m.from_block).count()
    }

    pub fn unsupported_total(&self) -> usize {
        self.unsupported.values().sum()
    }
}

/// Deduplicating texture pool used while baking.
#[derive(Default)]
pub struct TexturePool {
    textures: Vec<HeldTexture>,
    by_key: HashMap<String, u16>,
}

impl TexturePool {
    /// Intern a texture under `key`, calling `load` only on a miss. `None` when
    /// the texture cannot be decoded, which suppresses the item rather than
    /// drawing it untextured.
    pub fn intern(&mut self, key: &str, load: impl FnOnce() -> Option<HeldTexture>) -> Option<u16> {
        if let Some(&i) = self.by_key.get(key) {
            return Some(i);
        }
        let tex = load()?;
        let i = u16::try_from(self.textures.len()).ok()?;
        self.textures.push(tex);
        self.by_key.insert(key.to_string(), i);
        Some(i)
    }

    /// M244 — texture `id` with every texel's RGB multiplied by the sRGB
    /// colour `rgb` (alpha kept), interned once per (texture, colour). This
    /// is vanilla's per-vertex tint (a banner's dye) baked into a copy, for
    /// item paths whose vertices carry no colour. Multiplying the 8-bit sRGB
    /// values is exactly vanilla's gamma-space `texture * vertexColor`, and
    /// since a power curve is multiplicative it equals the world pass's
    /// linear-space multiply too.
    pub fn tinted(&mut self, id: u16, rgb: u32) -> Option<u16> {
        let key = format!("tint:{id}:{rgb:06x}");
        if let Some(&i) = self.by_key.get(&key) {
            return Some(i);
        }
        let src = self.textures.get(id as usize)?.clone();
        let mul = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF];
        self.intern(&key, || {
            let mut rgba = src.rgba;
            for px in rgba.as_chunks_mut::<4>().0 {
                for (c, m) in px.iter_mut().zip(mul) {
                    *c = ((*c as u32 * m + 127) / 255) as u8;
                }
            }
            Some(HeldTexture {
                w: src.w,
                h: src.h,
                rgba,
            })
        })
    }

    pub fn into_textures(self) -> Vec<HeldTexture> {
        self.textures
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }
}

#[cfg(test)]
mod tint_tests {
    use super::*;

    fn solid(w: u32, h: u32, px: [u8; 4]) -> HeldTexture {
        HeldTexture {
            w,
            h,
            rgba: px.repeat((w * h) as usize),
        }
    }

    /// M405 — a mask tinted by its dye blends over the base by its alpha:
    /// opaque replaces, half alpha mixes, clear leaves the base; and a
    /// sheet of another size refuses.
    #[test]
    fn layers_tint_and_blend_over_the_base() {
        let base = solid(2, 1, [200, 100, 50, 255]);
        let mut mask = solid(2, 1, [255, 255, 255, 255]);
        mask.rgba[7] = 128; // the second texel at half alpha
        let out = composite_layers(&base, &[(&mask, 0x80_40_20)]).unwrap();
        assert_eq!(&out.rgba[0..4], &[128, 64, 32, 255]);
        let half = |b: u32, t: u32| ((t * 128 + b * 127 + 127) / 255) as u8;
        assert_eq!(
            &out.rgba[4..7],
            &[half(200, 128), half(100, 64), half(50, 32)]
        );
        assert_eq!(out.rgba[7], 255);
        let clear = solid(2, 1, [255, 255, 255, 0]);
        assert_eq!(composite_layers(&base, &[(&clear, 0)]).unwrap(), base);
        assert!(composite_layers(&base, &[(&solid(1, 1, [0; 4]), 0)]).is_none());
    }

    fn quad(tex: u16) -> HeldQuad {
        HeldQuad {
            verts: [[0.0; 3]; 4],
            uv: [[0.0; 2]; 4],
            tex,
            part: 0,
            dir: 0,
        }
    }

    fn model(quads: Vec<HeldQuad>) -> HeldItemModel {
        HeldItemModel {
            quads,
            right: DisplayTransform::default(),
            left: DisplayTransform::default(),
            ground: DisplayTransform::default(),
            gui: DisplayTransform::default(),
            first_right: DisplayTransform::default(),
            first_left: DisplayTransform::default(),
            fixed: DisplayTransform::default(),
            head: DisplayTransform::default(),
            from_block: false,
            gui_quads: None,
        }
    }

    /// M407 — a pot copy repoints the k-th plain side quad at the k-th
    /// sherd's sheet (brick and an empty slot stay plain); four plain
    /// sides need no copy.
    #[test]
    fn a_pot_copy_wears_its_sherds_side_by_side() {
        let mut h = HeldItems::default();
        h.block_entities
            .insert("rewo:be/pot_side/plain".into(), model(vec![quad(1)]));
        h.block_entities
            .insert("rewo:be/pot_side/angler".into(), model(vec![quad(2)]));
        h.models.insert(
            "minecraft:decorated_pot".into(),
            model(vec![quad(0), quad(1), quad(1), quad(1), quad(1)]),
        );
        let made = h
            .add_pot_model([
                Some("minecraft:angler_pottery_sherd"),
                None,
                Some("minecraft:brick"),
                Some("minecraft:angler_pottery_sherd"),
            ])
            .unwrap();
        let texs: Vec<u16> = h.models[&made.name].quads.iter().map(|q| q.tex).collect();
        assert_eq!(texs, vec![0, 2, 1, 1, 2]);
        assert!(
            made.textures.is_empty(),
            "the placed pot's sheets are reused"
        );
        assert!(h
            .add_pot_model([None, Some("minecraft:brick"), None, None])
            .is_none());
    }

    /// M405 — a banner copy repoints only its flag (the tinted part) at
    /// the flag with the patterns over it; the pole keeps its sheet; one
    /// pattern list builds once. A shield copy draws `shield_base`, the
    /// tinted base (white without `base_color`) and its patterns; a plain
    /// shield needs no copy.
    #[test]
    fn a_pattern_copy_repoints_the_patterned_sheet() {
        let mut h = HeldItems {
            textures: vec![
                solid(2, 1, [10, 10, 10, 255]),
                solid(2, 1, [255, 255, 255, 255]),
            ],
            ..Default::default()
        };
        h.models.insert(
            "minecraft:white_banner".into(),
            model(vec![quad(0), quad(1)]),
        );
        h.tinted_parts
            .insert("minecraft:white_banner".into(), vec![1]);
        h.pattern_sheets.insert(
            "entity/banner/cross".into(),
            solid(2, 1, [255, 255, 255, 255]),
        );
        let layers = vec![("cross".to_string(), 14u8)];
        let made = h
            .add_pattern_model("minecraft:white_banner", None, &layers)
            .unwrap();
        assert_eq!(made.textures, vec![2]);
        let m = &h.models[&made.name];
        assert_eq!((m.quads[0].tex, m.quads[1].tex), (0, 2));
        // Red (14): 0xB02E26 over white.
        assert_eq!(&h.textures[2].rgba[0..3], &[0xB0, 0x2E, 0x26]);
        let again = h
            .add_pattern_model("minecraft:white_banner", None, &layers)
            .unwrap();
        assert_eq!((again.name, again.textures), (made.name, vec![]));

        h.models
            .insert("minecraft:shield".into(), model(vec![quad(0)]));
        for (p, px) in [
            ("entity/shield/shield_base", [9, 9, 9, 255]),
            ("entity/shield/base", [255, 255, 255, 255]),
        ] {
            h.pattern_sheets.insert(p.into(), solid(2, 1, px));
        }
        assert!(h.add_pattern_model("minecraft:shield", None, &[]).is_none());
        let s = h
            .add_pattern_model("minecraft:shield", Some(11), &[])
            .unwrap();
        let t = h.models[&s.name].quads[0].tex as usize;
        // Blue (11): 0x3C44AA, the base layer's dye, opaque over shield_base.
        assert_eq!(&h.textures[t].rgba[0..3], &[0x3C, 0x44, 0xAA]);
        assert!(h
            .add_pattern_model("minecraft:diamond_sword", None, &layers)
            .is_none());
    }

    /// A tinted copy multiplies RGB, keeps alpha, leaves the source alone,
    /// and is interned once per colour.
    #[test]
    fn a_tinted_copy_multiplies_rgb_and_keeps_alpha() {
        let mut pool = TexturePool::default();
        let src = pool
            .intern("src", || {
                Some(HeldTexture {
                    w: 1,
                    h: 1,
                    rgba: vec![255, 128, 0, 77],
                })
            })
            .unwrap();
        let red = pool.tinted(src, 0xB02E26).unwrap();
        assert_ne!(red, src);
        assert_eq!(pool.tinted(src, 0xB02E26), Some(red), "interned once");
        let tex = pool.into_textures();
        assert_eq!(tex[src as usize].rgba, vec![255, 128, 0, 77]);
        assert_eq!(tex[red as usize].rgba, vec![0xB0, 23, 0, 77]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(v: u8) -> HeldTexture {
        HeldTexture {
            w: 1,
            h: 1,
            rgba: vec![v, v, v, 255],
        }
    }

    #[test]
    fn the_pool_dedupes_by_key_and_loads_once() {
        let mut pool = TexturePool::default();
        let mut loads = 0;
        let a = pool.intern("x", || {
            loads += 1;
            Some(tex(1))
        });
        let b = pool.intern("x", || {
            loads += 1;
            Some(tex(2))
        });
        assert_eq!(a, Some(0));
        assert_eq!(b, Some(0), "the second intern must reuse, not re-add");
        assert_eq!(loads, 1, "the loader must not run on a hit");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn an_undecodable_texture_yields_none_and_adds_nothing() {
        let mut pool = TexturePool::default();
        assert_eq!(pool.intern("bad", || None), None);
        assert!(pool.is_empty(), "a failed load must not occupy a slot");
        // …and a later good load still gets slot 0.
        assert_eq!(pool.intern("good", || Some(tex(3))), Some(0));
    }

    #[test]
    fn counts_split_the_two_geometry_sources() {
        let mk = |from_block| HeldItemModel {
            gui: DisplayTransform::IDENTITY,
            quads: Vec::new(),
            right: DisplayTransform::default(),
            left: DisplayTransform::default(),
            ground: DisplayTransform::default(),
            first_right: DisplayTransform::default(),
            first_left: DisplayTransform::default(),
            fixed: DisplayTransform::default(),
            head: DisplayTransform::default(),
            from_block,
            gui_quads: None,
        };
        let items = HeldItems {
            models: [
                ("minecraft:dirt".to_string(), mk(true)),
                ("minecraft:stone".to_string(), mk(true)),
                ("minecraft:diamond_sword".to_string(), mk(false)),
            ]
            .into_iter()
            .collect(),
            block_entities: HashMap::new(),
            textures: Vec::new(),
            pattern_sheets: HashMap::new(),
            tinted_parts: HashMap::new(),
            unsupported: [("minecraft:special".to_string(), 51)]
                .into_iter()
                .collect(),
            dynamic: HashMap::new(),
        };
        assert_eq!(items.block_count(), 2);
        assert_eq!(items.sprite_count(), 1);
        assert_eq!(items.unsupported_total(), 51);
    }
}
