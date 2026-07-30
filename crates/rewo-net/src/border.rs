//! The six world-border packets (M80).
//!
//! | id | packet | body |
//! |---|---|---|
//! | 43 | `initialize_border` | the whole state at once |
//! | 88 | `set_border_center` | two `f64` |
//! | 89 | `set_border_lerp_size` | two `f64` + a **var-long** |
//! | 90 | `set_border_size` | one `f64` |
//! | 91 | `set_border_warning_delay` | one VarInt |
//! | 92 | `set_border_warning_distance` | one VarInt |
//!
//! The state machine they write is [`rewo_world::border::WorldBorder`]; this
//! module is only the wire and the five one-line handlers from
//! `ClientPacketListener`.
//!
//! # Three things that read backwards
//!
//! **`initialize_border` guards `lerpTime > 0` and `set_border_lerp_size` does
//! not.** `handleInitializeBorder` writes `if (lerpTime > 0L) lerpSizeBetween(…)
//! else setSize(newSize)`; `handleSetBorderLerpSize` calls `lerpSizeBetween`
//! flat. So a zero-duration lerp packet really does build a moving extent —
//! with an infinite lerp speed, for the one tick before it collapses. Adding
//! the guard to both looks like symmetry and changes what the warning vignette
//! does.
//!
//! **The two warning packets cross their names.** `set_border_warning_delay`
//! writes `warningTime` and `set_border_warning_distance` writes
//! `warningBlocks`; a reader that matches "distance" to "time" by position
//! decodes both without erroring, because both bodies are a single VarInt.
//! That is the same shape of silent mis-read M76 recorded for
//! `player_rotation`.
//!
//! **`lerpTime` is a var-long, not a var-int.** `readVarLong`. A duration past
//! 2³¹ ticks is absurd but legal, and a var-int reader would stop after five
//! bytes and then read the following VarInt fields out of the middle of it.

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;
use rewo_world::border::WorldBorder;

/// Which of the six arrived. Needed because the bodies do not distinguish
/// themselves: `set_border_warning_delay` and `set_border_warning_distance`
/// are both one VarInt, and only the packet id tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderPacket {
    Initialize,
    Center,
    LerpSize,
    Size,
    WarningDelay,
    WarningDistance,
}

/// The six resolved ids, so `kind_for_id` can be a pure function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BorderIds {
    pub initialize: i32,
    pub center: i32,
    pub lerp_size: i32,
    pub size: i32,
    pub warning_delay: i32,
    pub warning_distance: i32,
}

pub fn kind_for_id(id: i32, ids: BorderIds) -> Option<BorderPacket> {
    if id == ids.initialize {
        Some(BorderPacket::Initialize)
    } else if id == ids.center {
        Some(BorderPacket::Center)
    } else if id == ids.lerp_size {
        Some(BorderPacket::LerpSize)
    } else if id == ids.size {
        Some(BorderPacket::Size)
    } else if id == ids.warning_delay {
        Some(BorderPacket::WarningDelay)
    } else if id == ids.warning_distance {
        Some(BorderPacket::WarningDistance)
    } else {
        None
    }
}

/// `ClientboundInitializeBorderPacket`'s body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Initialize {
    pub center_x: f64,
    pub center_z: f64,
    /// The border's size *now*.
    pub old_size: f64,
    /// Its lerp target — equal to `old_size` when nothing is moving, because
    /// `StaticBorderExtent.getLerpTarget` returns its own size.
    pub new_size: f64,
    /// The ticks **remaining**, not the original duration: the server writes
    /// `border.getLerpTime()`, which is the live countdown.
    pub lerp_time: i64,
    pub absolute_max_size: i32,
    pub warning_blocks: i32,
    pub warning_time: i32,
}

pub fn read_initialize(r: &mut PacketReader<'_>) -> Result<Initialize> {
    Ok(Initialize {
        center_x: r.f64()?,
        center_z: r.f64()?,
        old_size: r.f64()?,
        new_size: r.f64()?,
        lerp_time: r.varlong()?,
        absolute_max_size: r.varint()?,
        warning_blocks: r.varint()?,
        warning_time: r.varint()?,
    })
}

/// `ClientboundSetBorderLerpSizePacket`'s body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LerpSize {
    pub old_size: f64,
    pub new_size: f64,
    pub lerp_time: i64,
}

pub fn read_lerp_size(r: &mut PacketReader<'_>) -> Result<LerpSize> {
    Ok(LerpSize {
        old_size: r.f64()?,
        new_size: r.f64()?,
        lerp_time: r.varlong()?,
    })
}

/// Apply one border packet. Returns whether the body decoded — distinct from
/// the router's "the id matched", the same split [`crate::view_area::apply`]
/// makes.
pub fn apply(kind: BorderPacket, body: &[u8], border: &mut WorldBorder) -> bool {
    let mut r = PacketReader::new(body);
    match kind {
        BorderPacket::Initialize => {
            let Ok(p) = read_initialize(&mut r) else {
                return false;
            };
            // `handleInitializeBorder`, in its order. The centre goes first
            // because both extents measure from it.
            border.set_center(p.center_x, p.center_z);
            if p.lerp_time > 0 {
                border.lerp_size_between(p.old_size, p.new_size, p.lerp_time);
            } else {
                // Note `newSize`, not `oldSize` — with no lerp running the
                // server has written the same number into both, and the target
                // is the one vanilla reads.
                border.set_size(p.new_size);
            }
            border.set_absolute_max_size(p.absolute_max_size);
            border.set_warning_blocks(p.warning_blocks);
            border.set_warning_time(p.warning_time);
            true
        }
        BorderPacket::Center => {
            let (Ok(x), Ok(z)) = (r.f64(), r.f64()) else {
                return false;
            };
            border.set_center(x, z);
            true
        }
        BorderPacket::LerpSize => {
            let Ok(p) = read_lerp_size(&mut r) else {
                return false;
            };
            // No `> 0` guard here — see the module doc.
            border.lerp_size_between(p.old_size, p.new_size, p.lerp_time);
            true
        }
        BorderPacket::Size => {
            let Ok(size) = r.f64() else {
                return false;
            };
            // `setSize` throws the running extent away, so this cancels a lerp.
            border.set_size(size);
            true
        }
        BorderPacket::WarningDelay => {
            let Ok(v) = r.varint() else {
                return false;
            };
            // "delay" → `setWarningTime`.
            border.set_warning_time(v);
            true
        }
        BorderPacket::WarningDistance => {
            let Ok(v) = r.varint() else {
                return false;
            };
            // "distance" → `setWarningBlocks`.
            border.set_warning_blocks(v);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::varint::{write_varint, write_varlong};
    use rewo_world::border::BorderStatus;

    const IDS: BorderIds = BorderIds {
        initialize: 43,
        center: 88,
        lerp_size: 89,
        size: 90,
        warning_delay: 91,
        warning_distance: 92,
    };

    fn init_body(
        cx: f64,
        cz: f64,
        old: f64,
        new: f64,
        lerp: i64,
        abs_max: i32,
        blocks: i32,
        time: i32,
    ) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [cx, cz, old, new] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        write_varlong(&mut b, lerp);
        write_varint(&mut b, abs_max);
        write_varint(&mut b, blocks);
        write_varint(&mut b, time);
        b
    }

    #[test]
    fn the_ids_select_the_right_packet_and_nothing_else() {
        assert_eq!(kind_for_id(43, IDS), Some(BorderPacket::Initialize));
        assert_eq!(kind_for_id(88, IDS), Some(BorderPacket::Center));
        assert_eq!(kind_for_id(89, IDS), Some(BorderPacket::LerpSize));
        assert_eq!(kind_for_id(90, IDS), Some(BorderPacket::Size));
        assert_eq!(kind_for_id(91, IDS), Some(BorderPacket::WarningDelay));
        assert_eq!(kind_for_id(92, IDS), Some(BorderPacket::WarningDistance));
        assert_eq!(kind_for_id(44, IDS), None);
    }

    #[test]
    fn initialize_carries_the_whole_state() {
        let body = init_body(100.0, -50.0, 64.0, 64.0, 0, 29_999_984, 7, 300);
        let mut b = WorldBorder::default();
        assert!(apply(BorderPacket::Initialize, &body, &mut b));
        assert_eq!(b.center_x(), 100.0);
        assert_eq!(b.center_z(), -50.0);
        assert_eq!(b.size(), 64.0);
        assert_eq!(b.status(), BorderStatus::Stationary);
        assert_eq!(b.absolute_max_size(), 29_999_984);
        assert_eq!(b.warning_blocks(), 7);
        assert_eq!(b.warning_time(), 300);
        // The centre took effect, so the box is around it, not the origin.
        assert_eq!(b.min_x(0.0), 68.0);
        assert_eq!(b.max_z(0.0), -18.0);
    }

    #[test]
    fn initialize_resumes_a_lerp_that_was_already_running() {
        // A player joining mid-shrink gets the *remaining* ticks.
        let body = init_body(0.0, 0.0, 180.0, 100.0, 40, 29_999_984, 5, 300);
        let mut b = WorldBorder::default();
        assert!(apply(BorderPacket::Initialize, &body, &mut b));
        assert_eq!(b.size(), 180.0);
        assert_eq!(b.status(), BorderStatus::Shrinking);
        assert_eq!(b.lerp_time(), 40);
        for _ in 0..40 {
            b.tick();
        }
        assert_eq!(b.size(), 100.0);
    }

    #[test]
    fn a_zero_lerp_time_takes_initializes_static_branch() {
        let body = init_body(0.0, 0.0, 500.0, 64.0, 0, 29_999_984, 5, 300);
        let mut b = WorldBorder::default();
        apply(BorderPacket::Initialize, &body, &mut b);
        // `setSize(newSize)`, so the 500 is discarded outright.
        assert_eq!(b.size(), 64.0);
        assert_eq!(b.status(), BorderStatus::Stationary);
    }

    #[test]
    fn set_border_lerp_size_has_no_zero_guard() {
        // The asymmetry with `initialize_border` above: the same numbers on
        // this packet build a *moving* extent instead of a static one.
        let mut body = Vec::new();
        body.extend_from_slice(&500.0f64.to_be_bytes());
        body.extend_from_slice(&64.0f64.to_be_bytes());
        write_varlong(&mut body, 0);
        let mut b = WorldBorder::default();
        assert!(apply(BorderPacket::LerpSize, &body, &mut b));
        assert_eq!(b.status(), BorderStatus::Shrinking, "moving, not static");
        assert_eq!(b.size(), 64.0);
        assert!(b.lerp_speed().is_infinite());
    }

    #[test]
    fn the_lerp_time_is_a_var_long() {
        // 2^32 ticks: five bytes of var-int would stop short and the following
        // fields would be read out of the middle of it.
        let mut body = Vec::new();
        body.extend_from_slice(&1.0f64.to_be_bytes());
        body.extend_from_slice(&2.0f64.to_be_bytes());
        write_varlong(&mut body, 1 << 32);
        let mut r = PacketReader::new(&body);
        let p = read_lerp_size(&mut r).expect("decodes");
        assert_eq!(p.lerp_time, 1 << 32);
        assert!(r.is_empty(), "and consumed the whole body");
    }

    #[test]
    fn set_border_size_cancels_a_running_lerp() {
        let mut b = WorldBorder::default();
        b.lerp_size_between(1000.0, 100.0, 1000);
        b.tick();
        apply(BorderPacket::Size, &256.0f64.to_be_bytes(), &mut b);
        assert_eq!(b.size(), 256.0);
        assert_eq!(b.status(), BorderStatus::Stationary);
    }

    #[test]
    fn the_two_warning_packets_write_the_fields_their_names_cross_to() {
        let mut b = WorldBorder::default();
        let mut delay = Vec::new();
        write_varint(&mut delay, 123);
        let mut distance = Vec::new();
        write_varint(&mut distance, 45);
        apply(BorderPacket::WarningDelay, &delay, &mut b);
        apply(BorderPacket::WarningDistance, &distance, &mut b);
        assert_eq!(b.warning_time(), 123, "delay → time");
        assert_eq!(b.warning_blocks(), 45, "distance → blocks");
    }

    #[test]
    fn a_short_body_is_reported_rather_than_half_applied() {
        let mut b = WorldBorder::default();
        let before = b;
        assert!(!apply(BorderPacket::Center, &[0u8; 4], &mut b));
        assert!(!apply(BorderPacket::Initialize, &[0u8; 8], &mut b));
        assert!(!apply(BorderPacket::Size, &[], &mut b));
        assert_eq!(b, before, "nothing was written from a truncated body");
    }

    #[test]
    fn the_center_moves_the_box_without_touching_the_size() {
        let mut b = WorldBorder::default();
        b.set_size(20.0);
        let mut body = Vec::new();
        body.extend_from_slice(&1000.0f64.to_be_bytes());
        body.extend_from_slice(&(-1000.0f64).to_be_bytes());
        assert!(apply(BorderPacket::Center, &body, &mut b));
        assert_eq!(b.size(), 20.0);
        assert_eq!(b.min_x(0.0), 990.0);
        assert_eq!(b.max_z(0.0), -990.0);
    }
}
