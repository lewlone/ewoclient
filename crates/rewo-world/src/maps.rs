//! The client's map store (M190) — `ClientLevel`'s `MapItemSavedData` per
//! `MapId`, fed by `ClientboundMapItemDataPacket`.
//!
//! `ClientPacketListener.handleMapItemData`:
//!
//! ```java
//! MapItemSavedData data = this.minecraft.level.getMapData(id);
//! if (data == null) {
//!    data = MapItemSavedData.createForClient(packet.scale(), packet.locked(), dimension);
//!    this.minecraft.level.overrideMapData(id, data);
//! }
//! packet.applyToMap(data);
//! this.minecraft.getMapTextureManager().update(id, data);
//! ```
//!
//! Three things in that shape are easy to get wrong and are pinned by the
//! tests below:
//!
//! - **`scale` and `locked` are read only when the map is created.** A later
//!   packet for the same id that says otherwise changes nothing about them —
//!   `applyToMap` touches decorations and colours alone.
//! - **Decorations are replaced wholesale** when the packet carries a list
//!   (`addClientSideDecorations` clears first), and left alone when it carries
//!   none (`Optional.empty`). An empty list is not "none": it clears them.
//! - **Colours are a patch**, `colors[(startX + x) + (startY + y) * 128] =
//!   mapColors[x + y * width]`; everything outside the patch keeps its value.
//!   A fresh map is all zeros, which is `MapColor.NONE`, transparent.
//!
//! The store lives on the level: a new `ClientLevel` (login, respawn into
//! another dimension) starts with no maps, and the server resends what the
//! player holds or sees. [`MapStore::clear`] is that.

use std::collections::HashMap;

use rewo_proto::nbt::Nbt;

/// `MapItemSavedData`'s side: 128 x 128 pixels.
pub const MAP_SIZE: usize = 128;

/// One `MapDecoration` as the client receives it.
#[derive(Clone, Debug, PartialEq)]
pub struct MapDecoration {
    /// The `minecraft:map_decoration_type` registry id (`holderRegistry`: raw,
    /// 0-based), resolved to a name through the report by whoever draws it.
    pub type_id: i32,
    /// Map-local position in half-pixels, `-128..=127` (two per pixel, centre
    /// at 0).
    pub x: i8,
    pub y: i8,
    /// `rot & 15` — the record's compact constructor masks it, sixteenths of a
    /// turn.
    pub rot: u8,
    /// `Optional<Component>`, the banner's name on a map.
    pub name: Option<Nbt>,
}

/// One decoded `ClientboundMapItemDataPacket`.
#[derive(Clone, Debug, PartialEq)]
pub struct MapItemData {
    pub map_id: i32,
    pub scale: i8,
    pub locked: bool,
    /// `None` = the packet carries no decoration list (leave them); `Some`
    /// replaces them, an empty list included.
    pub decorations: Option<Vec<MapDecoration>>,
    pub patch: Option<MapPatch>,
}

/// `MapItemSavedData.MapPatch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapPatch {
    pub start_x: u8,
    pub start_y: u8,
    pub width: u8,
    pub height: u8,
    /// `width * height` packed colours, column-fastest (`x + y * width`).
    pub colors: Vec<u8>,
}

impl MapPatch {
    /// Whether `applyToMap` would stay inside the array. Vanilla's loop
    /// indexes `mapColors[x + y * width]` and `colors[(startX + x) + (startY +
    /// y) * 128]` with no bounds check, so a short array or a patch running
    /// off the map throws inside the handler; Rewo refuses the packet whole
    /// instead (the decode counts it), which is the same outcome without the
    /// half-applied map.
    pub fn fits(&self) -> bool {
        let (w, h) = (self.width as usize, self.height as usize);
        self.colors.len() >= w * h
            && self.start_x as usize + w <= MAP_SIZE
            && self.start_y as usize + h <= MAP_SIZE
    }
}

/// One map's client state — `MapItemSavedData.createForClient`'s fields that
/// the client reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientMap {
    pub scale: i8,
    pub locked: bool,
    /// `colors`, `x + y * 128`, packed `MapColor` ids.
    pub colors: Vec<u8>,
    pub decorations: Vec<MapDecoration>,
    /// Bumped whenever the colours change, so a renderer re-uploads only a
    /// map that moved (`MapTextureManager.update`'s dirty flag).
    pub version: u64,
}

impl ClientMap {
    fn new(scale: i8, locked: bool) -> Self {
        Self {
            scale,
            locked,
            colors: vec![0; MAP_SIZE * MAP_SIZE],
            decorations: Vec::new(),
            version: 0,
        }
    }

    /// The map as RGBA bytes, row-major from the top-left —
    /// `MapTextureManager.MapInstance.updateTextureIfNeeded`'s
    /// `pixels.setPixel(x, y, MapColor.getColorFromPackedId(colors[i]))`.
    pub fn rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAP_SIZE * MAP_SIZE * 4);
        for &c in &self.colors {
            let argb = rewo_data::map_colors::argb_from_packed(c);
            out.extend_from_slice(&[
                (argb >> 16) as u8,
                (argb >> 8) as u8,
                argb as u8,
                (argb >> 24) as u8,
            ]);
        }
        out
    }
}

/// Every map the level holds, by `MapId`.
#[derive(Clone, Debug, Default)]
pub struct MapStore {
    maps: HashMap<i32, ClientMap>,
    /// Packets applied, for witnesses.
    pub applied: u64,
}

impl MapStore {
    /// `handleMapItemData`. The caller has already refused a patch that does
    /// not [`MapPatch::fits`].
    pub fn apply(&mut self, data: &MapItemData) {
        let map = self
            .maps
            .entry(data.map_id)
            .or_insert_with(|| ClientMap::new(data.scale, data.locked));
        if let Some(decorations) = &data.decorations {
            map.decorations.clone_from(decorations);
        }
        if let Some(p) = &data.patch {
            let w = p.width as usize;
            for y in 0..p.height as usize {
                for x in 0..w {
                    let at = (p.start_x as usize + x) + (p.start_y as usize + y) * MAP_SIZE;
                    map.colors[at] = p.colors[x + y * w];
                }
            }
            map.version += 1;
        }
        self.applied += 1;
    }

    pub fn get(&self, map_id: i32) -> Option<&ClientMap> {
        self.maps.get(&map_id)
    }

    pub fn len(&self) -> usize {
        self.maps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.maps.is_empty()
    }

    /// A new `ClientLevel`: no maps.
    pub fn clear(&mut self) {
        self.maps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(
        id: i32,
        patch: Option<MapPatch>,
        decorations: Option<Vec<MapDecoration>>,
    ) -> MapItemData {
        MapItemData {
            map_id: id,
            scale: 0,
            locked: false,
            decorations,
            patch,
        }
    }

    fn deco(type_id: i32) -> MapDecoration {
        MapDecoration {
            type_id,
            x: 0,
            y: 0,
            rot: 0,
            name: None,
        }
    }

    #[test]
    fn a_patch_writes_its_rectangle_and_nothing_else() {
        let mut s = MapStore::default();
        // 2 x 3 at (10, 20): colours 1..=6, x fastest.
        let p = MapPatch {
            start_x: 10,
            start_y: 20,
            width: 2,
            height: 3,
            colors: vec![1, 2, 3, 4, 5, 6],
        };
        assert!(p.fits());
        s.apply(&data(7, Some(p), None));
        let m = s.get(7).unwrap();
        assert_eq!(m.colors[10 + 20 * 128], 1);
        assert_eq!(m.colors[11 + 20 * 128], 2);
        assert_eq!(m.colors[10 + 21 * 128], 3, "row 1 starts at index width");
        assert_eq!(m.colors[11 + 22 * 128], 6);
        assert_eq!(m.colors.iter().filter(|&&c| c != 0).count(), 6);
        assert_eq!(m.version, 1);
    }

    #[test]
    fn scale_and_lock_are_fixed_at_creation() {
        let mut s = MapStore::default();
        let mut first = data(1, None, None);
        first.scale = 2;
        first.locked = true;
        s.apply(&first);
        let mut second = data(1, None, None);
        second.scale = 4;
        second.locked = false;
        s.apply(&second);
        let m = s.get(1).unwrap();
        assert_eq!((m.scale, m.locked), (2, true));
        assert_eq!(m.version, 0, "no patch, no colour change");
    }

    #[test]
    fn decorations_are_replaced_by_a_list_and_kept_without_one() {
        let mut s = MapStore::default();
        s.apply(&data(1, None, Some(vec![deco(0), deco(3)])));
        assert_eq!(s.get(1).unwrap().decorations.len(), 2);
        s.apply(&data(1, None, None));
        assert_eq!(
            s.get(1).unwrap().decorations.len(),
            2,
            "Optional.empty keeps them"
        );
        s.apply(&data(1, None, Some(vec![])));
        assert!(
            s.get(1).unwrap().decorations.is_empty(),
            "an empty list clears them"
        );
    }

    #[test]
    fn a_patch_that_runs_off_the_map_or_its_array_does_not_fit() {
        let p = |sx, w, len| MapPatch {
            start_x: sx,
            start_y: 0,
            width: w,
            height: 1,
            colors: vec![0; len],
        };
        assert!(p(0, 128, 128).fits());
        assert!(!p(1, 128, 128).fits(), "x 1 + 128 > 128");
        assert!(!p(0, 4, 3).fits(), "short colour array");
    }

    /// The texture is `MapColor.getColorFromPackedId` per pixel: id 0 is
    /// transparent, and GRASS (id 1, 0x7FB238) at NORMAL (brightness 1, x220)
    /// is `0x7F*220/255, 0xB2*220/255, 0x38*220/255` = (109, 153, 48).
    #[test]
    fn rgba_is_the_palette_per_pixel() {
        let mut s = MapStore::default();
        let p = MapPatch {
            start_x: 0,
            start_y: 0,
            width: 2,
            height: 1,
            colors: vec![1 << 2 | 1, 1 << 2 | 2],
        };
        s.apply(&data(9, Some(p), None));
        let rgba = s.get(9).unwrap().rgba();
        assert_eq!(rgba.len(), 128 * 128 * 4);
        assert_eq!(&rgba[0..4], &[109, 153, 48, 255]);
        assert_eq!(
            &rgba[4..8],
            &[0x7F, 0xB2, 0x38, 255],
            "HIGH is x255: the base colour"
        );
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0], "NONE is transparent");
    }

    /// Ids past the declared materials are `byIdUnsafe`'s NONE; LOWEST is x135.
    #[test]
    fn undeclared_ids_are_transparent_and_lowest_is_135() {
        use rewo_data::map_colors::{argb_from_packed, BRIGHTNESS, MATERIALS};
        assert_eq!(BRIGHTNESS, [180, 220, 255, 135]);
        assert_eq!(MATERIALS.len(), 62);
        assert_eq!(argb_from_packed(62 << 2 | 2), 0);
        assert_eq!(argb_from_packed(63 << 2 | 1), 0);
        // SNOW (8, 0xFFFFFF) at LOWEST: 255 * 135 / 255 = 135.
        assert_eq!(argb_from_packed(8 << 2 | 3), 0xFF87_8787);
    }

    #[test]
    fn a_new_level_has_no_maps() {
        let mut s = MapStore::default();
        s.apply(&data(1, None, None));
        assert_eq!(s.len(), 1);
        s.clear();
        assert!(s.is_empty());
    }
}
