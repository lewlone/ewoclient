//! Boss bars — `ClientboundBossEventPacket` (M65).
//!
//! **Decode + state only.** A boss bar is drawn 182 px wide at the top of the
//! screen from a sprite sheet; none of that is here, and the visual freeze
//! (`REWO_VELVET_UI_PLAN.md` §8) is why.
//!
//! ## The shape that makes this packet different
//!
//! It is an **operation-tagged union**: a UUID, an operation ordinal, and then
//! a body whose fields depend entirely on which operation the ordinal named.
//! `REMOVE` has no body at all. So the ordinal cannot be read-and-discarded
//! and the body cannot be read unconditionally — the same trap `stop_sound`'s
//! flag byte sets in M63, one level up.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/network/protocol/game/ClientboundBossEventPacket.java` —
//!   the six operations and their bodies
//! - `net/minecraft/client/gui/components/BossHealthOverlay.java` —
//!   `update(packet)`, which is the entire client-side state machine
//! - `net/minecraft/client/gui/components/LerpingBossEvent.java`
//! - `net/minecraft/world/BossEvent.java` — the colour and overlay enums

use std::collections::HashMap;

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::{ProtoError, Result};

/// `BossEvent.BossBarColor`, read by `input.readEnum`.
///
/// `readEnum` indexes `getEnumConstants()`, so an out-of-range ordinal throws
/// in vanilla and is an error here — not a clamp to `PINK`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BossBarColor {
    Pink,
    Blue,
    Red,
    Green,
    Yellow,
    Purple,
    White,
}

impl BossBarColor {
    pub const ALL: [BossBarColor; 7] = [
        BossBarColor::Pink,
        BossBarColor::Blue,
        BossBarColor::Red,
        BossBarColor::Green,
        BossBarColor::Yellow,
        BossBarColor::Purple,
        BossBarColor::White,
    ];

    pub fn from_ordinal(id: i32) -> Option<BossBarColor> {
        usize::try_from(id).ok().and_then(|i| Self::ALL.get(i)).copied()
    }

    fn read(r: &mut PacketReader<'_>) -> Result<BossBarColor> {
        let ordinal = r.varint()?;
        BossBarColor::from_ordinal(ordinal).ok_or(ProtoError::LengthOutOfRange {
            what: "boss bar colour ordinal",
            len: ordinal as i64,
            max: Self::ALL.len() - 1,
        })
    }

    /// `getSerializedName`. Not on the wire — for logs and a future HUD, which
    /// picks its sprite by this name.
    pub fn name(self) -> &'static str {
        match self {
            BossBarColor::Pink => "pink",
            BossBarColor::Blue => "blue",
            BossBarColor::Red => "red",
            BossBarColor::Green => "green",
            BossBarColor::Yellow => "yellow",
            BossBarColor::Purple => "purple",
            BossBarColor::White => "white",
        }
    }
}

/// `BossEvent.BossBarOverlay` — the notch pattern drawn over the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BossBarOverlay {
    Progress,
    Notched6,
    Notched10,
    Notched12,
    Notched20,
}

impl BossBarOverlay {
    pub const ALL: [BossBarOverlay; 5] = [
        BossBarOverlay::Progress,
        BossBarOverlay::Notched6,
        BossBarOverlay::Notched10,
        BossBarOverlay::Notched12,
        BossBarOverlay::Notched20,
    ];

    pub fn from_ordinal(id: i32) -> Option<BossBarOverlay> {
        usize::try_from(id).ok().and_then(|i| Self::ALL.get(i)).copied()
    }

    fn read(r: &mut PacketReader<'_>) -> Result<BossBarOverlay> {
        let ordinal = r.varint()?;
        BossBarOverlay::from_ordinal(ordinal).ok_or(ProtoError::LengthOutOfRange {
            what: "boss bar overlay ordinal",
            len: ordinal as i64,
            max: Self::ALL.len() - 1,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BossBarOverlay::Progress => "progress",
            BossBarOverlay::Notched6 => "notched_6",
            BossBarOverlay::Notched10 => "notched_10",
            BossBarOverlay::Notched12 => "notched_12",
            BossBarOverlay::Notched20 => "notched_20",
        }
    }
}

/// The three world-effect flags, packed into one unsigned byte.
///
/// Kept packed on the wire and unpacked here because `encodeProperties` is the
/// only writer and it is three independent bits — an enum would imply they are
/// exclusive, which they are not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BossBarProperties {
    /// `FLAG_DARKEN` (1) — dim the sky, the Wither/Dragon effect.
    pub darken_screen: bool,
    /// `FLAG_MUSIC` (2) — play the boss music track.
    pub play_music: bool,
    /// `FLAG_FOG` (4) — thicken world fog.
    pub create_world_fog: bool,
}

impl BossBarProperties {
    /// Vanilla tests `(flags & N) > 0`, so bits 3..7 are simply ignored rather
    /// than rejected.
    pub fn from_bits(flags: u8) -> BossBarProperties {
        BossBarProperties {
            darken_screen: flags & 1 > 0,
            play_music: flags & 2 > 0,
            create_world_fog: flags & 4 > 0,
        }
    }

    fn read(r: &mut PacketReader<'_>) -> Result<BossBarProperties> {
        Ok(BossBarProperties::from_bits(r.u8()?))
    }
}

/// `ClientboundBossEventPacket.OperationType` and its body, together.
///
/// One enum rather than an ordinal plus an optional body, because the two are
/// not independent: there is exactly one body shape per ordinal, and pairing
/// them in the type is what stops a caller reading a progress from a remove.
#[derive(Clone, Debug, PartialEq)]
pub enum BossEventOp {
    /// 0 — create or replace.
    Add {
        /// `ComponentSerialization.TRUSTED_STREAM_CODEC` — one network-NBT tag.
        name: Nbt,
        progress: f32,
        color: BossBarColor,
        overlay: BossBarOverlay,
        properties: BossBarProperties,
    },
    /// 1 — **no body at all**. Reading one would consume the next packet's
    /// first bytes if this were not framed.
    Remove,
    /// 2 — a bare `float`, big-endian, not a var-int.
    UpdateProgress(f32),
    /// 3.
    UpdateName(Nbt),
    /// 4.
    UpdateStyle {
        color: BossBarColor,
        overlay: BossBarOverlay,
    },
    /// 5.
    UpdateProperties(BossBarProperties),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BossEventPacket {
    /// `input.readUUID()` — two big-endian longs, not a string.
    pub id: u128,
    pub op: BossEventOp,
}

impl BossEventPacket {
    pub fn read(r: &mut PacketReader<'_>) -> Result<BossEventPacket> {
        let id = r.uuid()?;
        // `input.readEnum(OperationType.class)` — the same array index as the
        // colour enums, so an unknown operation is an error. It has to be:
        // the ordinal is what says how long the body is, so there is nothing
        // to skip and nothing safe to assume.
        let ordinal = r.varint()?;
        let op = match ordinal {
            0 => BossEventOp::Add {
                name: r.nbt()?,
                progress: r.f32()?,
                color: BossBarColor::read(r)?,
                overlay: BossBarOverlay::read(r)?,
                properties: BossBarProperties::read(r)?,
            },
            1 => BossEventOp::Remove,
            2 => BossEventOp::UpdateProgress(r.f32()?),
            3 => BossEventOp::UpdateName(r.nbt()?),
            4 => BossEventOp::UpdateStyle {
                color: BossBarColor::read(r)?,
                overlay: BossBarOverlay::read(r)?,
            },
            5 => BossEventOp::UpdateProperties(BossBarProperties::read(r)?),
            other => {
                return Err(ProtoError::LengthOutOfRange {
                    what: "boss event operation ordinal",
                    len: other as i64,
                    max: 5,
                })
            }
        };
        Ok(BossEventPacket { id, op })
    }
}

pub fn parse_boss_event(body: &[u8]) -> Result<BossEventPacket> {
    BossEventPacket::read(&mut PacketReader::new(body))
}

// ── The state ─────────────────────────────────────────────────────────────

/// One bar as `BossHealthOverlay` holds it.
///
/// `progress` is the value the server last set. Vanilla's `LerpingBossEvent`
/// eases towards it over 100 ms and `getProgress()` returns the eased value —
/// deliberately not modelled, because that is a *render-time* read off a
/// wall clock and there is no renderer. A HUD that wants the ease can hold
/// its own previous value; the wire state is this number.
#[derive(Clone, Debug, PartialEq)]
pub struct BossBar {
    pub name: Nbt,
    pub progress: f32,
    pub color: BossBarColor,
    pub overlay: BossBarOverlay,
    pub properties: BossBarProperties,
}

/// `BossHealthOverlay.events` — a `LinkedHashMap`, so **insertion order is
/// display order** and the top-to-bottom stack of bars is the order the
/// server added them in.
#[derive(Debug, Default, Clone)]
pub struct BossBars {
    /// The `LinkedHashMap` key order. A separate vector rather than an ordered
    /// map because the only two mutations are push-if-new and remove.
    order: Vec<u128>,
    bars: HashMap<u128, BossBar>,
}

impl BossBars {
    pub fn new() -> BossBars {
        BossBars::default()
    }

    /// `BossHealthOverlay::update`.
    ///
    /// Returns false when the packet named a bar that does not exist. Vanilla
    /// does **not** check: `events.get(id).setProgress(...)` is a
    /// NullPointerException on the render thread for every operation but ADD
    /// and REMOVE. A server does not send one, so this is a documented
    /// divergence in the shape M62 records for `removePlayerFromTeam`: log,
    /// change nothing.
    pub fn apply(&mut self, p: &BossEventPacket) -> bool {
        match &p.op {
            BossEventOp::Add {
                name,
                progress,
                color,
                overlay,
                properties,
            } => {
                // `Map::put` — a repeat ADD replaces the value and, being a
                // LinkedHashMap, keeps its ORIGINAL position in the stack.
                // Pushing the id again would move the bar down the screen on
                // a re-send, which vanilla never does.
                if !self.bars.contains_key(&p.id) {
                    self.order.push(p.id);
                }
                self.bars.insert(
                    p.id,
                    BossBar {
                        name: name.clone(),
                        progress: *progress,
                        color: *color,
                        overlay: *overlay,
                        properties: *properties,
                    },
                );
                true
            }
            BossEventOp::Remove => {
                if self.bars.remove(&p.id).is_none() {
                    return false;
                }
                self.order.retain(|id| *id != p.id);
                true
            }
            op => {
                let Some(bar) = self.bars.get_mut(&p.id) else {
                    log::debug!("play: boss_event {op:?} for unknown bar {:032x}", p.id);
                    return false;
                };
                match op {
                    BossEventOp::UpdateProgress(progress) => bar.progress = *progress,
                    BossEventOp::UpdateName(name) => bar.name = name.clone(),
                    BossEventOp::UpdateStyle { color, overlay } => {
                        bar.color = *color;
                        bar.overlay = *overlay;
                    }
                    BossEventOp::UpdateProperties(properties) => bar.properties = *properties,
                    // Handled above; matched exhaustively so a new operation
                    // cannot silently fall through to "no effect".
                    BossEventOp::Add { .. } | BossEventOp::Remove => unreachable!(),
                }
                true
            }
        }
    }

    /// The bars in display order.
    pub fn iter(&self) -> impl Iterator<Item = (u128, &BossBar)> {
        self.order.iter().filter_map(|id| self.bars.get(id).map(|b| (*id, b)))
    }

    pub fn get(&self, id: u128) -> Option<&BossBar> {
        self.bars.get(&id)
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    /// `BossHealthOverlay::reset` — called on world change, not by a packet.
    pub fn clear(&mut self) {
        self.order.clear();
        self.bars.clear();
    }

    /// `shouldDarkenScreen` — **any** bar asking is enough.
    pub fn should_darken_screen(&self) -> bool {
        self.bars.values().any(|b| b.properties.darken_screen)
    }

    /// `BossHealthOverlay.shouldPlayMusic()` — any bar asking for boss music.
    ///
    /// **Its one caller gates it on the dimension** (M146):
    /// `getSituationalMusic` tests `dimension() == Level.END` *first*, so a
    /// wither fought in the Overworld sets this flag and still does not get
    /// `Musics.END_BOSS`.
    pub fn should_play_music(&self) -> bool {
        self.bars.values().any(|b| b.properties.play_music)
    }

    /// `shouldCreateWorldFog`.
    pub fn should_create_world_fog(&self) -> bool {
        self.bars.values().any(|b| b.properties.create_world_fog)
    }
}

#[cfg(test)]
mod tests {
    //! Bodies built by hand, run through the real `BossEventPacket::read` and
    //! the real `BossBars::apply`.

    use super::*;

    const SENTINEL: u8 = 0xA7;

    fn varint(out: &mut Vec<u8>, mut v: i32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v = ((v as u32) >> 7) as i32;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                break;
            }
        }
    }

    fn nbt_string(out: &mut Vec<u8>, s: &str) {
        out.push(8);
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn header(out: &mut Vec<u8>, id: u128, operation: i32) {
        out.extend_from_slice(&id.to_be_bytes());
        varint(out, operation);
    }

    fn add_body(id: u128, name: &str, progress: f32, color: i32, overlay: i32, flags: u8) -> Vec<u8> {
        let mut b = Vec::new();
        header(&mut b, id, 0);
        nbt_string(&mut b, name);
        b.extend_from_slice(&progress.to_be_bytes());
        varint(&mut b, color);
        varint(&mut b, overlay);
        b.push(flags);
        b
    }

    fn remove_body(id: u128) -> Vec<u8> {
        let mut b = Vec::new();
        header(&mut b, id, 1);
        b
    }

    /// Prove a decode consumed exactly `body`: an under-read leaves more than
    /// one byte, an over-read eats the sentinel or runs off the end.
    fn exact(body: &[u8]) -> BossEventPacket {
        let mut with_sentinel = body.to_vec();
        with_sentinel.push(SENTINEL);
        let mut r = PacketReader::new(&with_sentinel);
        let p = BossEventPacket::read(&mut r).expect("body decodes");
        assert_eq!(r.remaining(), 1, "decode must stop at the sentinel");
        assert_eq!(r.u8().unwrap(), SENTINEL);
        p
    }

    const A: u128 = 0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10;
    const B: u128 = 0x1111_2222_3333_4444_5555_6666_7777_8888;

    // ── The wire ──────────────────────────────────────────────────────────

    #[test]
    fn an_add_carries_five_fields_and_consumes_exactly_them() {
        let p = exact(&add_body(A, "Wither", 0.75, 2, 4, 7));
        assert_eq!(p.id, A);
        let BossEventOp::Add {
            name,
            progress,
            color,
            overlay,
            properties,
        } = p.op
        else {
            panic!("operation 0 is ADD");
        };
        assert_eq!(name, Nbt::String("Wither".into()));
        assert_eq!(progress, 0.75);
        assert_eq!(color, BossBarColor::Red);
        assert_eq!(overlay, BossBarOverlay::Notched20);
        assert_eq!(
            properties,
            BossBarProperties {
                darken_screen: true,
                play_music: true,
                create_world_fog: true
            }
        );
    }

    #[test]
    fn a_remove_has_no_body_at_all() {
        // The operation the union exists for. If REMOVE read anything, the
        // sentinel would be consumed as part of it.
        let p = exact(&remove_body(A));
        assert_eq!(p.op, BossEventOp::Remove);
    }

    #[test]
    fn each_update_operation_reads_only_its_own_fields() {
        let mut progress = Vec::new();
        header(&mut progress, A, 2);
        progress.extend_from_slice(&0.5f32.to_be_bytes());
        assert_eq!(exact(&progress).op, BossEventOp::UpdateProgress(0.5));

        let mut name = Vec::new();
        header(&mut name, A, 3);
        nbt_string(&mut name, "Ender Dragon");
        assert_eq!(
            exact(&name).op,
            BossEventOp::UpdateName(Nbt::String("Ender Dragon".into()))
        );

        let mut style = Vec::new();
        header(&mut style, A, 4);
        varint(&mut style, 5);
        varint(&mut style, 1);
        assert_eq!(
            exact(&style).op,
            BossEventOp::UpdateStyle {
                color: BossBarColor::Purple,
                overlay: BossBarOverlay::Notched6
            }
        );

        let mut props = Vec::new();
        header(&mut props, A, 5);
        props.push(2);
        assert_eq!(
            exact(&props).op,
            BossEventOp::UpdateProperties(BossBarProperties {
                darken_screen: false,
                play_music: true,
                create_world_fog: false
            })
        );
    }

    #[test]
    fn the_progress_is_a_big_endian_float_not_a_var_int() {
        // A var-int reader would consume one byte of 0.5f32 (0x3F 00 00 00),
        // report 63, and leave three bytes of it in the buffer. `exact`
        // catches exactly that.
        let mut b = Vec::new();
        header(&mut b, A, 2);
        b.extend_from_slice(&0.5f32.to_be_bytes());
        assert_eq!(b.len(), 16 + 1 + 4);
        assert_eq!(exact(&b).op, BossEventOp::UpdateProgress(0.5));
    }

    #[test]
    fn the_id_is_two_big_endian_longs_and_not_a_string() {
        let p = exact(&remove_body(B));
        assert_eq!(p.id, B);
    }

    #[test]
    fn an_unknown_operation_is_an_error_because_its_body_length_is_unknowable() {
        let mut b = Vec::new();
        header(&mut b, A, 6);
        assert!(parse_boss_event(&b).is_err());
    }

    #[test]
    fn an_out_of_range_colour_or_overlay_is_an_error_rather_than_the_zeroth() {
        // `readEnum`, not `ByIdMap.continuous` — the opposite of the display
        // slot in the scoreboard packets.
        assert_eq!(BossBarColor::from_ordinal(7), None);
        assert_eq!(BossBarOverlay::from_ordinal(5), None);
        assert!(parse_boss_event(&add_body(A, "x", 1.0, 7, 0, 0)).is_err());
        assert!(parse_boss_event(&add_body(A, "x", 1.0, 0, 5, 0)).is_err());
    }

    #[test]
    fn property_bits_above_the_third_are_ignored_rather_than_rejected() {
        // `(flags & N) > 0` never looks at the high bits.
        assert_eq!(
            BossBarProperties::from_bits(0xF8),
            BossBarProperties::default()
        );
        assert_eq!(
            BossBarProperties::from_bits(0xFF),
            BossBarProperties {
                darken_screen: true,
                play_music: true,
                create_world_fog: true
            }
        );
    }

    #[test]
    fn a_truncated_add_is_an_error_rather_than_a_half_bar() {
        let full = add_body(A, "Wither", 0.75, 2, 4, 7);
        assert!(parse_boss_event(&full[..full.len() - 1]).is_err());
    }

    // ── The state ─────────────────────────────────────────────────────────

    #[test]
    fn bars_render_in_the_order_they_were_added() {
        let mut bars = BossBars::new();
        bars.apply(&parse_boss_event(&add_body(A, "first", 1.0, 0, 0, 0)).unwrap());
        bars.apply(&parse_boss_event(&add_body(B, "second", 1.0, 0, 0, 0)).unwrap());
        let names: Vec<_> = bars.iter().map(|(_, b)| b.name.clone()).collect();
        assert_eq!(
            names,
            [Nbt::String("first".into()), Nbt::String("second".into())]
        );
    }

    #[test]
    fn re_adding_a_bar_replaces_it_without_moving_it_down_the_stack() {
        // `LinkedHashMap::put` on an existing key does not re-order. Pushing
        // the id again would swap the two bars on screen.
        let mut bars = BossBars::new();
        bars.apply(&parse_boss_event(&add_body(A, "first", 1.0, 0, 0, 0)).unwrap());
        bars.apply(&parse_boss_event(&add_body(B, "second", 1.0, 0, 0, 0)).unwrap());
        bars.apply(&parse_boss_event(&add_body(A, "first again", 0.5, 1, 0, 0)).unwrap());

        let rows: Vec<_> = bars.iter().map(|(id, b)| (id, b.name.clone())).collect();
        assert_eq!(
            rows,
            [
                (A, Nbt::String("first again".into())),
                (B, Nbt::String("second".into()))
            ]
        );
        assert_eq!(bars.len(), 2);
        assert_eq!(bars.get(A).unwrap().color, BossBarColor::Blue);
    }

    #[test]
    fn removing_a_bar_drops_it_from_both_the_map_and_the_order() {
        let mut bars = BossBars::new();
        bars.apply(&parse_boss_event(&add_body(A, "first", 1.0, 0, 0, 0)).unwrap());
        bars.apply(&parse_boss_event(&add_body(B, "second", 1.0, 0, 0, 0)).unwrap());
        assert!(bars.apply(&parse_boss_event(&remove_body(A)).unwrap()));
        assert_eq!(bars.iter().count(), 1);
        assert!(bars.get(A).is_none());
        // And a re-add now goes to the back, because it really was gone.
        bars.apply(&parse_boss_event(&add_body(A, "first", 1.0, 0, 0, 0)).unwrap());
        assert_eq!(bars.iter().map(|(id, _)| id).collect::<Vec<_>>(), [B, A]);
    }

    #[test]
    fn an_update_for_an_unknown_bar_changes_nothing_instead_of_panicking() {
        // Vanilla NPEs here. The requirement is only that nothing else moves.
        let mut bars = BossBars::new();
        bars.apply(&parse_boss_event(&add_body(A, "first", 1.0, 0, 0, 0)).unwrap());
        let mut b = Vec::new();
        header(&mut b, B, 2);
        b.extend_from_slice(&0.25f32.to_be_bytes());
        assert!(!bars.apply(&parse_boss_event(&b).unwrap()));
        assert_eq!(bars.get(A).unwrap().progress, 1.0);
        assert_eq!(bars.len(), 1);
    }

    #[test]
    fn a_style_update_sets_both_colour_and_overlay() {
        let mut bars = BossBars::new();
        bars.apply(&parse_boss_event(&add_body(A, "x", 1.0, 0, 0, 0)).unwrap());
        let mut b = Vec::new();
        header(&mut b, A, 4);
        varint(&mut b, 6);
        varint(&mut b, 3);
        assert!(bars.apply(&parse_boss_event(&b).unwrap()));
        let bar = bars.get(A).unwrap();
        assert_eq!(bar.color, BossBarColor::White);
        assert_eq!(bar.overlay, BossBarOverlay::Notched12);
        // and leaves the fields it does not name alone
        assert_eq!(bar.progress, 1.0);
        assert_eq!(bar.name, Nbt::String("x".into()));
    }

    #[test]
    fn the_world_effects_are_any_not_all() {
        let mut bars = BossBars::new();
        // one bar with music only, one with nothing
        bars.apply(&parse_boss_event(&add_body(A, "x", 1.0, 0, 0, 2)).unwrap());
        bars.apply(&parse_boss_event(&add_body(B, "y", 1.0, 0, 0, 0)).unwrap());
        assert!(bars.should_play_music());
        assert!(!bars.should_darken_screen());
        assert!(!bars.should_create_world_fog());

        // and the flags follow a properties update
        let mut b = Vec::new();
        header(&mut b, B, 5);
        b.push(5);
        bars.apply(&parse_boss_event(&b).unwrap());
        assert!(bars.should_darken_screen());
        assert!(bars.should_create_world_fog());

        // removing the only bar that asked takes the effect with it
        bars.apply(&parse_boss_event(&remove_body(B)).unwrap());
        assert!(!bars.should_darken_screen());
        assert!(bars.should_play_music());
    }

    #[test]
    fn removing_an_unknown_bar_is_inert() {
        let mut bars = BossBars::new();
        assert!(!bars.apply(&parse_boss_event(&remove_body(A)).unwrap()));
        assert!(bars.is_empty());
    }
}
