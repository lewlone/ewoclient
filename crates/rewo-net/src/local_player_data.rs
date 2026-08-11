//! The local player's own `SynchedEntityData`, and the elytra sound's rising
//! edge (M141e).
//!
//! # Why this exists at all
//!
//! `handleSetEntityData` is `if (entity != null)`, and vanilla's local player
//! **is** in the level — so the server's metadata for you is processed exactly
//! like anyone else's. Rewo's [`rewo_world::entities::EntityTable`] holds only
//! entities the server sent an `add_entity` for, and it never sends one for
//! you, so [`crate::apply_set_entity_data`] returns early on your own id and
//! everything in it is dropped.
//!
//! That is M73's asymmetry — the same one that made
//! `entity_interaction_range` permanently the registered default until the
//! attribute path grew a local copy — and the fix has the same shape: decode
//! the body a second time when it names the camera entity, and keep the
//! result beside the table.
//!
//! # What it unblocks
//!
//! `isFallFlying()` is `getSharedFlag(7)` (`LivingEntity.java:3653`,
//! `Entity.FLAG_FALL_FLYING = 7`), and it is the elytra sound's input at
//! **both ends**:
//!
//! * the **trigger** — `LocalPlayer.onSyncedDataUpdated` plays
//!   `ElytraOnPlayerSoundInstance` on the rising edge;
//! * the **ramp's survival guard** — `time <= 20 || isFallFlying()`, so
//!   without it the sound would play for exactly one second and stop.
//!
//! One decode closes both, which is why they ship together.
//!
//! # The edge is not "the flag changed"
//!
//! ```java
//! if (DATA_SHARED_FLAGS_ID.equals(accessor) && this.isFallFlying() && !this.wasFallFlying) {
//!    this.minecraft.getSoundManager().play(new ElytraOnPlayerSoundInstance(this));
//! }
//! ```
//! (`LocalPlayer.java:591-593`.)
//!
//! Three separate conditions, and the natural simplification of any of them
//! diverges:
//!
//! 1. **The packet must carry index 0.** `SynchedEntityData.assignValues`
//!    calls `onSyncedDataUpdated(accessor)` once per *entry in the packet*
//!    (`:109-113`), so a metadata update that does not mention the shared
//!    flags cannot fire this however the flag stands.
//! 2. **There is no change guard.** `assignValues` fires the callback
//!    unconditionally — it does not compare the new value with the old. So a
//!    packet re-sending flags that already had bit 7 set *does* reach the
//!    test.
//! 3. **`wasFallFlying` is sampled once per tick**, in `aiStep`
//!    (`LocalPlayer.java:854`), not updated by the callback. So what stops
//!    (2) from starting a sound on every packet is the tick sample, and
//!    **two flag-carrying packets inside one tick each start one** — vanilla
//!    has no dedup in `SoundEngine.play`, so you get two overlapping elytra
//!    loops. That is a quirk rather than a bug to fix, and it is witnessed
//!    below because "did the flag change?" is the obvious implementation and
//!    would silently not do it.

use rewo_data::components::DataComponentIds;
use rewo_proto::reader::PacketReader;

/// `Entity.FLAG_FALL_FLYING`.
pub const FLAG_FALL_FLYING: u8 = 7;

/// The local player's synced data, kept beside the entity table.
///
/// Only the shared flags so far: they are what the sound path needs, and a
/// field nothing reads would be indistinguishable from a field nothing
/// *writes*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalPlayerData {
    /// `DATA_SHARED_FLAGS_ID` — index 0, BYTE.
    shared_flags: u8,
    /// `LocalPlayer.wasFallFlying`, sampled once per `aiStep`.
    was_fall_flying: bool,
}

impl LocalPlayerData {
    /// `getSharedFlag(flag)`.
    pub fn shared_flag(&self, flag: u8) -> bool {
        self.shared_flags & (1 << flag) != 0
    }

    /// `LivingEntity.isFallFlying()` — `getSharedFlag(7)`.
    pub fn is_fall_flying(&self) -> bool {
        self.shared_flag(FLAG_FALL_FLYING)
    }

    /// The raw byte, for a caller that wants another flag later.
    pub fn shared_flags(&self) -> u8 {
        self.shared_flags
    }

    /// `aiStep`'s `this.wasFallFlying = this.isFallFlying();`
    ///
    /// Once per **tick**, and that placement is the whole reason the rising
    /// edge terminates — see the module doc's point (3).
    pub fn tick(&mut self) {
        self.was_fall_flying = self.is_fall_flying();
    }

    pub fn was_fall_flying(&self) -> bool {
        self.was_fall_flying
    }
}

/// What one `set_entity_data` naming the local player did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalMetaOutcome {
    /// The packet carried `DATA_SHARED_FLAGS_ID`, so
    /// `onSyncedDataUpdated(DATA_SHARED_FLAGS_ID)` ran.
    pub flags_updated: bool,
    /// …and the elytra sound should start.
    pub start_elytra_sound: bool,
}

/// `handleSetEntityData` for the local player — the branch
/// [`crate::apply_set_entity_data`] cannot take because the table has no row.
///
/// Returns `LocalMetaOutcome::default()` for a body that does not parse or
/// names anything but the camera entity, so a caller can run it on every
/// `set_entity_data` unconditionally, exactly as M73's
/// `apply_local_attributes` is run.
pub fn apply_local_metadata(
    body: &[u8],
    player_id: Option<i32>,
    components: Option<DataComponentIds>,
    data: &mut LocalPlayerData,
) -> LocalMetaOutcome {
    let Some(player_id) = player_id else {
        return LocalMetaOutcome::default();
    };
    let mut r = PacketReader::new(body);
    let Ok(eid) = r.varint() else {
        return LocalMetaOutcome::default();
    };
    if eid != player_id {
        return LocalMetaOutcome::default();
    }
    let meta = crate::metadata::parse(&mut r, components);
    let Some(flags) = meta.flags else {
        // The packet did not mention index 0, so `onSyncedDataUpdated` never
        // ran for that accessor and the elytra test is not reached — however
        // the flag stands.
        return LocalMetaOutcome::default();
    };
    data.shared_flags = flags;
    LocalMetaOutcome {
        flags_updated: true,
        // No change guard: `assignValues` fires the callback for every entry
        // in the packet, so this is `isFallFlying() && !wasFallFlying` on the
        // freshly-assigned value.
        start_elytra_sound: data.is_fall_flying() && !data.was_fall_flying,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// A `set_entity_data` body: entity id, then one BYTE entry at index 0,
    /// then the terminator.
    fn flags_body(eid: i32, flags: u8) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.varint(eid);
        w.u8(0); // index 0 — DATA_SHARED_FLAGS_ID
        w.varint(0); // serializer 0 — BYTE
        w.u8(flags);
        w.u8(0xFF); // terminator
        w.into_bytes()
    }

    /// A body carrying an entry that is NOT the shared flags: index 3,
    /// BOOLEAN (`DATA_CUSTOM_NAME_VISIBLE`).
    fn other_body(eid: i32) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.varint(eid);
        w.u8(3);
        w.varint(8); // BOOLEAN
        w.u8(1);
        w.u8(0xFF);
        w.into_bytes()
    }

    const FLYING: u8 = 1 << FLAG_FALL_FLYING;

    #[test]
    fn the_flag_is_bit_seven_and_nothing_else() {
        let mut d = LocalPlayerData::default();
        assert!(!d.is_fall_flying());
        apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        assert!(d.is_fall_flying());
        assert_eq!(d.shared_flags(), 0x80);

        // Every other bit leaves it alone — on fire, sneaking, sprinting,
        // swimming, invisible and glowing all live in the same byte.
        let mut d = LocalPlayerData::default();
        apply_local_metadata(&flags_body(1, 0x7F), Some(1), None, &mut d);
        assert!(!d.is_fall_flying(), "0x7F is every bit but 7");
    }

    /// **The rising edge fires once**, because `wasFallFlying` is sampled per
    /// tick.
    #[test]
    fn the_elytra_edge_fires_once_per_takeoff() {
        let mut d = LocalPlayerData::default();
        let out = apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        assert!(out.start_elytra_sound, "the take-off");
        assert!(out.flags_updated);

        // The tick samples it…
        d.tick();
        // …and a later packet with the flag still set does not fire again.
        let out = apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        assert!(!out.start_elytra_sound);
        assert!(out.flags_updated, "the accessor still updated");
    }

    /// **Two flag-carrying packets inside ONE tick both fire**, because the
    /// sample that stops it is the tick's, not the callback's — and vanilla's
    /// `SoundEngine.play` has no dedup, so you really do get two overlapping
    /// loops.
    ///
    /// This is the witness for the module doc's point (3): "did the flag
    /// change?" is the obvious implementation and would fire once.
    #[test]
    fn two_packets_in_one_tick_both_fire() {
        let mut d = LocalPlayerData::default();
        let a = apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        let b = apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        assert!(a.start_elytra_sound && b.start_elytra_sound);
    }

    /// **A packet that does not mention the shared flags cannot fire it**,
    /// however the flag stands — `assignValues` calls the callback once per
    /// entry, and the guard is on the accessor.
    #[test]
    fn a_packet_without_the_flags_entry_is_inert() {
        let mut d = LocalPlayerData::default();
        // Take off, then let the tick sample it so the edge is spent.
        apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        d.tick();
        // Now land and take off again — but in a packet carrying only index 3.
        let out = apply_local_metadata(&other_body(1), Some(1), None, &mut d);
        assert!(!out.flags_updated);
        assert!(!out.start_elytra_sound);
        assert!(d.is_fall_flying(), "and the flag is untouched");
    }

    /// Landing clears it, and the next take-off fires again — the edge is
    /// re-armable rather than a latch.
    #[test]
    fn landing_re_arms_the_edge() {
        let mut d = LocalPlayerData::default();
        apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        d.tick();
        apply_local_metadata(&flags_body(1, 0), Some(1), None, &mut d);
        assert!(!d.is_fall_flying());
        d.tick();
        let out = apply_local_metadata(&flags_body(1, FLYING), Some(1), None, &mut d);
        assert!(out.start_elytra_sound, "a second take-off fires again");
    }

    /// Anything but the camera entity changes nothing — the remote path owns
    /// those, and double-applying them here would be a second writer.
    #[test]
    fn another_entitys_metadata_is_ignored() {
        let mut d = LocalPlayerData::default();
        let out = apply_local_metadata(&flags_body(2, FLYING), Some(1), None, &mut d);
        assert_eq!(out, LocalMetaOutcome::default());
        assert_eq!(d, LocalPlayerData::default());

        // …and so is every body before the login packet names us.
        let out = apply_local_metadata(&flags_body(1, FLYING), None, None, &mut d);
        assert_eq!(out, LocalMetaOutcome::default());
        assert_eq!(d, LocalPlayerData::default());
    }

    /// **`Bee.DATA_ANGER_END_TIME` is index 19 and serializer 2** (M141f),
    /// driven through the production `apply_set_entity_data` rather than the
    /// parser alone, so the kind gate and the table write are graded too.
    ///
    /// The index is Bee's *second* own accessor: Entity 0..7, LivingEntity
    /// 8..14, Mob 15, **AgeableMob 16 AND 17** — `DATA_BABY_ID` and the
    /// `AGE_LOCKED` that is easy to miss — Animal none, Bee 18..19. Counting
    /// AgeableMob as one puts this on `Bee.DATA_FLAGS_ID`.
    #[test]
    fn the_bees_anger_deadline_is_index_nineteen_long() {
        use rewo_proto::writer::PacketWriter;
        let mut t = rewo_world::entities::EntityTable::default();
        // type id 7 stands in for `minecraft:bee` and is what the gate is told.
        t.add(
            9,
            rewo_world::entities::EntityState::new(0, 7, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        let mut w = PacketWriter::default();
        w.varint(9);
        w.u8(19);
        w.varint(2); // LONG — a VAR_LONG, not a fixed i64
        w.varlong(1234);
        w.u8(0xFF);
        let body = w.into_bytes();

        let kinds = |bee: Option<i32>| crate::MetaKinds {
            allay: None,
            pillager: None,
            sheep: None,
            creaking: None,
            player: None,
            bee,
            variant_kinds: Default::default(),
            classes: None,
            components: None,
        };

        crate::apply_set_entity_data(&body, &mut t, kinds(Some(7)));
        assert_eq!(t.anger_end_time(9), Some(1234));

        // **The kind gate is load-bearing**: index 19 is Bee's own slot, and
        // another class's nineteenth accessor could be a LONG too. A table
        // told the wrong kind must not store it.
        let mut t2 = rewo_world::entities::EntityTable::default();
        t2.add(
            9,
            rewo_world::entities::EntityState::new(0, 7, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        crate::apply_set_entity_data(&body, &mut t2, kinds(Some(8)));
        assert_eq!(t2.anger_end_time(9), None, "wrong kind, no write");
        crate::apply_set_entity_data(&body, &mut t2, kinds(None));
        assert_eq!(t2.anger_end_time(9), None, "no kind, no write");
    }

    /// A body that does not parse changes nothing rather than panicking — the
    /// caller runs this on every `set_entity_data`.
    #[test]
    fn a_truncated_body_is_inert() {
        let mut d = LocalPlayerData::default();
        for body in [&[][..], &[0x80][..], &[0x01][..]] {
            let out = apply_local_metadata(body, Some(1), None, &mut d);
            assert_eq!(out, LocalMetaOutcome::default());
        }
        assert_eq!(d, LocalPlayerData::default());
    }
}
