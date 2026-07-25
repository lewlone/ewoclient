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
    /// True when the geometry came from a block model — recorded so the
    /// renderer and the gate can tell the two sources apart without
    /// re-deriving it.
    pub from_block: bool,
}

/// Every resolvable item's baked model, keyed by full registry name.
#[derive(Clone, Debug, Default)]
pub struct HeldItems {
    pub models: HashMap<String, HeldItemModel>,
    pub textures: Vec<HeldTexture>,
    /// Definition types that were deliberately not resolved, and how many
    /// items each accounted for. Sorted so the load log is stable.
    pub unsupported: BTreeMap<String, usize>,
}

impl HeldItems {
    pub fn get(&self, name: &str) -> Option<&HeldItemModel> {
        self.models.get(name)
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
    pub fn intern(
        &mut self,
        key: &str,
        load: impl FnOnce() -> Option<HeldTexture>,
    ) -> Option<u16> {
        if let Some(&i) = self.by_key.get(key) {
            return Some(i);
        }
        let tex = load()?;
        let i = u16::try_from(self.textures.len()).ok()?;
        self.textures.push(tex);
        self.by_key.insert(key.to_string(), i);
        Some(i)
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
            quads: Vec::new(),
            right: DisplayTransform::default(),
            left: DisplayTransform::default(),
            ground: DisplayTransform::default(),
            from_block,
        };
        let items = HeldItems {
            models: [
                ("minecraft:dirt".to_string(), mk(true)),
                ("minecraft:stone".to_string(), mk(true)),
                ("minecraft:diamond_sword".to_string(), mk(false)),
            ]
            .into_iter()
            .collect(),
            textures: Vec::new(),
            unsupported: [("minecraft:special".to_string(), 51)].into_iter().collect(),
        };
        assert_eq!(items.block_count(), 2);
        assert_eq!(items.sprite_count(), 1);
        assert_eq!(items.unsupported_total(), 51);
    }
}
