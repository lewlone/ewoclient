//! `ClientboundMapItemDataPacket` (M190) — the wire half of maps. The store
//! it feeds is `rewo_world::maps`.
//!
//! ```java
//! StreamCodec.composite(
//!    MapId.STREAM_CODEC,                    // VAR_INT
//!    ByteBufCodecs.BYTE,                    // scale
//!    ByteBufCodecs.BOOL,                    // locked
//!    MapDecoration.STREAM_CODEC.apply(list()).apply(optional),
//!    MapItemSavedData.MapPatch.STREAM_CODEC, // its own optional, see below
//!    ...)
//! ```
//!
//! - A decoration is `holderRegistry(MAP_DECORATION_TYPE)` — a **raw** 0-based
//!   registry id, not `holder`'s `id + 1` — then three bytes and
//!   `ComponentSerialization.OPTIONAL_STREAM_CODEC` (a bool, then one NBT tag).
//!   The record masks `rot & 15`.
//! - The patch is **not** `ByteBufCodecs.optional`: its absence is a single
//!   `width == 0` byte, and only a non-zero width is followed by height,
//!   startX, startY and a VarInt-prefixed byte array.
//!
//! A body is refused whole (and counted by the caller) when it is truncated,
//! carries bytes past its end, or holds a patch that `applyToMap` would index
//! out of bounds — vanilla throws inside the handler for that last one.

use rewo_proto::reader::PacketReader;
use rewo_world::maps::{MapDecoration, MapItemData, MapPatch};

/// Decode one `map_item_data` body.
pub fn read_map_item_data(body: &[u8]) -> Result<MapItemData, String> {
    let mut r = PacketReader::new(body);
    let e = |e: rewo_proto::ProtoError| e.to_string();
    let map_id = r.varint().map_err(e)?;
    let scale = r.i8().map_err(e)?;
    let locked = r.bool().map_err(e)?;
    let decorations = if r.bool().map_err(e)? {
        // type (>= 1 byte) + x + y + rot + the name's presence bool.
        let n = r.count("map decorations", 5).map_err(e)?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let type_id = r.varint().map_err(e)?;
            let x = r.i8().map_err(e)?;
            let y = r.i8().map_err(e)?;
            let rot = r.u8().map_err(e)? & 15;
            let name = if r.bool().map_err(e)? {
                Some(r.nbt().map_err(e)?)
            } else {
                None
            };
            out.push(MapDecoration {
                type_id,
                x,
                y,
                rot,
                name,
            });
        }
        Some(out)
    } else {
        None
    };
    let width = r.u8().map_err(e)?;
    let patch = if width > 0 {
        let height = r.u8().map_err(e)?;
        let start_x = r.u8().map_err(e)?;
        let start_y = r.u8().map_err(e)?;
        let remaining = r.remaining();
        let colors = r.byte_array(remaining).map_err(e)?.to_vec();
        let p = MapPatch {
            start_x,
            start_y,
            width,
            height,
            colors,
        };
        if !p.fits() {
            return Err(format!(
                "map {map_id}: a {width}x{height} patch at ({start_x}, {start_y}) with {} colours \
                 does not fit a 128x128 map",
                p.colors.len()
            ));
        }
        Some(p)
    } else {
        None
    };
    if !r.is_empty() {
        return Err(format!("map {map_id}: {} trailing bytes", r.remaining()));
    }
    Ok(MapItemData {
        map_id,
        scale,
        locked,
        decorations,
        patch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    fn body(build: impl FnOnce(&mut PacketWriter)) -> Vec<u8> {
        let mut w = PacketWriter::default();
        build(&mut w);
        w.into_bytes()
    }

    #[test]
    fn a_full_patch_with_no_decorations_decodes() {
        let b = body(|w| {
            w.varint(42);
            w.i8(3);
            w.bool(true);
            w.bool(false); // no decoration list
            w.u8(2); // width
            w.u8(1); // height
            w.u8(126); // start x
            w.u8(127); // start y
            w.varint(2);
            w.raw(&[0x05, 0x0A]);
        });
        let d = read_map_item_data(&b).unwrap();
        assert_eq!((d.map_id, d.scale, d.locked), (42, 3, true));
        assert_eq!(d.decorations, None);
        let p = d.patch.unwrap();
        assert_eq!((p.start_x, p.start_y, p.width, p.height), (126, 127, 2, 1));
        assert_eq!(p.colors, vec![5, 10]);
    }

    /// Width 0 is the whole of an absent patch: one byte, nothing after it.
    #[test]
    fn a_zero_width_is_no_patch_and_one_byte() {
        let b = body(|w| {
            w.varint(1);
            w.i8(0);
            w.bool(false);
            w.bool(false);
            w.u8(0);
        });
        assert_eq!(read_map_item_data(&b).unwrap().patch, None);
        let mut longer = b.clone();
        longer.push(0);
        assert!(
            read_map_item_data(&longer).is_err(),
            "a trailing byte is refused"
        );
    }

    /// The type id is raw (`holderRegistry`), the rotation masked to 0..15,
    /// and the name an optional NBT component.
    #[test]
    fn decorations_read_raw_ids_masked_rotation_and_an_optional_name() {
        let b = body(|w| {
            w.varint(5);
            w.i8(0);
            w.bool(false);
            w.bool(true);
            w.varint(2); // two decorations
            w.varint(0); // player: raw id 0, not 0 + 1
            w.i8(-128);
            w.i8(127);
            w.u8(0x1F); // rot 31 -> 15
            w.bool(false);
            w.varint(9);
            w.i8(4);
            w.i8(-4);
            w.u8(3);
            w.bool(true);
            w.raw(&[8, 0, 2, b'h', b'i']); // an NBT string tag, "hi"
            w.u8(0); // no patch
        });
        let d = read_map_item_data(&b).unwrap();
        let ds = d.decorations.unwrap();
        assert_eq!(ds.len(), 2);
        assert_eq!(
            (ds[0].type_id, ds[0].x, ds[0].y, ds[0].rot),
            (0, -128, 127, 15)
        );
        assert!(ds[0].name.is_none());
        assert_eq!((ds[1].type_id, ds[1].rot), (9, 3));
        assert!(ds[1].name.is_some());
    }

    #[test]
    fn a_patch_off_the_map_or_short_of_colours_is_refused() {
        let patch = |sx: u8, w_: u8, n: usize| {
            body(|w| {
                w.varint(1);
                w.i8(0);
                w.bool(false);
                w.bool(false);
                w.u8(w_);
                w.u8(1);
                w.u8(sx);
                w.u8(0);
                w.varint(n as i32);
                w.raw(&vec![1; n]);
            })
        };
        assert!(read_map_item_data(&patch(0, 128, 128)).is_ok());
        assert!(
            read_map_item_data(&patch(1, 128, 128)).is_err(),
            "runs off x"
        );
        assert!(
            read_map_item_data(&patch(0, 4, 3)).is_err(),
            "3 colours for 4"
        );
        let mut cut = patch(0, 4, 4);
        cut.pop();
        assert!(read_map_item_data(&cut).is_err(), "truncated");
    }
}
