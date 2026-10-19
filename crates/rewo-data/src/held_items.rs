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
    pub fn add_skin_head_model(&mut self, rgba: Vec<u8>) -> Option<(String, usize)> {
        if rgba.len() != 64 * 64 * 4 {
            return None;
        }
        let mut model = self.any("rewo:be/player_head")?.clone();
        let tex = self.textures.len();
        let t = u16::try_from(tex).ok()?;
        self.textures.push(HeldTexture { w: 64, h: 64, rgba });
        for q in &mut model.quads {
            q.tex = t;
        }
        let name = format!("rewo:be/player_head#{tex}");
        self.block_entities.insert(name.clone(), model);
        Some((name, tex))
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
