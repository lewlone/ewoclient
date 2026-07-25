//! `ItemStack.OPTIONAL_STREAM_CODEC` — decoded exactly as far as a combat
//! swing needs, and **fail-closed** everywhere it cannot be (M19).
//!
//! Wire form (26.2 `ItemStack.createOptionalStreamCodec`):
//!
//! ```text
//! VarInt count            // <= 0 → ItemStack.EMPTY, nothing follows
//! VarInt item             // Item.STREAM_CODEC = holderRegistry(ITEM) → raw registry id
//! DataComponentPatch      // VarInt added, VarInt removed,
//!                         //   added:   VarInt component type id + value (per-type codec)
//!                         //   removed: VarInt component type id
//! ```
//!
//! **Why the patch matters at all.** The value the client actually reads is
//! `getOrDefault(SWING_ANIMATION, SwingAnimation.DEFAULT)` over the item's
//! prototype components *patched* by this delta. For every vanilla item the
//! prototype answers it (see `rewo_data::swing_anim`), but the component is
//! `networkSynchronized`, so a datapack/plugin server can override or remove it
//! per stack, and the client would honour that.
//!
//! **Two independent things can go wrong, and they are tracked separately.**
//!
//! 1. *Alignment.* Each added component's value is encoded with its own stream
//!    codec, so skipping one requires knowing that codec. This decoder
//!    transcribes exactly three of the 111 registered component codecs —
//!    `minecraft:swing_animation`, `minecraft:damage` (a bare VarInt, and by
//!    far the most common thing a vanilla server patches onto a held weapon)
//!    and `minecraft:charged_projectiles` (M23, a nested list of item
//!    templates, each with its own patch — so the walk recurses, bounded).
//!    The first entry it cannot walk leaves the reader parked mid-value:
//!    [`PatchOutcome::Unwalkable`], and **the enclosing packet must stop** —
//!    every later slot would be parsed out of garbage.
//! 2. *Knowledge.* Even a fully-walked patch can leave the swing animation
//!    unknowable — an item id the registry does not contain has no prototype.
//!
//! Neither case is ever converted into a bare/prototype/default guess.
//! [`resolve_swing`] returns [`SwingResolution::Unknown`], and the caller
//! suppresses the combat pose and CEM `swing_progress` for that entity until an
//! exact equipment update repairs it.
//!
//! Note the walk continues *past* the swing component: finding it early does
//! not license returning early, because the entries after it still have to be
//! consumed for the reader to be aligned for the next slot.

use rewo_data::components::DataComponentIds;
use rewo_data::swing_anim::{SwingAnimation, SwingAnimationType, SwingAnimations};
use rewo_data::use_item::{UseProfile, UseProfiles};
use rewo_proto::reader::PacketReader;

/// Everything the equipment decoder needs that is resolved once, from the
/// datagen reports, before the session starts: the item → prototype swing
/// animation table and the data-component registry ids the patch is keyed by.
pub struct SwingWireData {
    pub prototypes: SwingAnimations,
    pub components: DataComponentIds,
    /// Item → `getUseDuration` / `getUseAnimation` (M23). Resolved from the
    /// same reports at the same moment as `prototypes`, and for the same
    /// reason: neither value is on the wire.
    pub use_profiles: UseProfiles,
}

/// What a fully-walked `DataComponentPatch` said about
/// `minecraft:swing_animation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchSwing {
    /// The patch did not mention the component.
    Absent,
    /// The patch set it explicitly.
    Set(SwingAnimation),
    /// The patch *removed* it (`!swing_animation`). `PatchedDataComponentMap`
    /// then returns null and `getOrDefault` hands back `SwingAnimation.DEFAULT`
    /// — which is **not** the item prototype: a spear with the component
    /// removed swings like a fist.
    Removed,
}

/// The result of walking one stack's patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchOutcome {
    /// Every added and removed entry was consumed; the reader is aligned on
    /// whatever follows the stack.
    Walked(PatchSwing),
    /// An entry with an un-transcribed codec was reached. The reader is parked
    /// mid-value, the swing animation is unknowable, and the enclosing packet
    /// cannot be read any further.
    Unwalkable,
}

/// One decoded slot value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireSlot {
    /// `count <= 0` → `ItemStack.EMPTY` (nothing else is encoded).
    Empty,
    Stack(WireStack),
}

impl WireSlot {
    /// Whether the reader is positioned on the next value. `false` means the
    /// caller must abandon the rest of the packet.
    pub fn aligned(&self) -> bool {
        match self {
            WireSlot::Empty => true,
            WireSlot::Stack(s) => matches!(s.patch, PatchOutcome::Walked(_)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireStack {
    pub count: i32,
    /// Item registry protocol id, exactly as sent — validated in
    /// [`resolve_swing`], not here.
    pub item_id: i32,
    pub patch: PatchOutcome,
    /// What the patch said about `minecraft:charged_projectiles` (M23).
    ///
    /// Tracked beside [`Self::patch`] rather than inside it because the two
    /// answer different questions and fail independently: the swing resolution
    /// is about an item's *prototype*, this is about a per-stack override that
    /// only ever arrives as a patch.
    pub charged: PatchCharged,
}

impl WireStack {
    /// Whether this stack's patch was fully walked, so the reader is aligned
    /// on whatever follows it.
    pub fn aligned_stack(&self) -> bool {
        matches!(self.patch, PatchOutcome::Walked(_))
    }
}

/// What a stack's `DataComponentPatch` said about
/// `minecraft:charged_projectiles`.
///
/// The value is reduced to "is the projectile list non-empty", because that is
/// all `CrossbowItem.isCharged` asks:
/// `!getOrDefault(CHARGED_PROJECTILES, ChargedProjectiles.EMPTY).isEmpty()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PatchCharged {
    /// The patch did not mention the component, so the item prototype answers.
    /// Every vanilla prototype is `ChargedProjectiles.EMPTY`, so this reads as
    /// *not charged*.
    #[default]
    Absent,
    /// The patch set it: `true` when the list has at least one projectile.
    Set(bool),
    /// The patch *removed* it. `getOrDefault` then hands back
    /// `ChargedProjectiles.EMPTY` — not charged.
    Removed,
}

impl PatchCharged {
    /// `CrossbowItem.isCharged(stack)`. `Absent` and `Removed` both resolve
    /// through `ChargedProjectiles.EMPTY`, so both are `false`.
    pub const fn is_charged(self) -> bool {
        matches!(self, PatchCharged::Set(true))
    }
}

/// Why a stack's swing animation could not be resolved exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownSwing {
    /// The patch held a component whose codec this decoder does not transcribe.
    /// Anything it might have overridden is invisible.
    UnwalkableComponent,
    /// The item id is not in the registry, so it has no known prototype
    /// components.
    UnregisteredItem,
}

/// The value `ItemStack.getSwingAnimation()` would return — or an explicit
/// statement that it cannot be known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwingResolution {
    Exact(SwingAnimation),
    Unknown(UnknownSwing),
}

/// Decode one `ItemStack.OPTIONAL_STREAM_CODEC` value.
///
/// `Err(())` is a truncated body: the reader is then in an undefined position,
/// so callers must abandon the rest of the packet. A successfully returned
/// [`WireSlot`] may still be un[`aligned`](WireSlot::aligned) — check it.
#[allow(clippy::result_unit_err)]
pub fn read_optional(r: &mut PacketReader, ids: DataComponentIds) -> Result<WireSlot, ()> {
    let count = r.varint().map_err(|_| ())?;
    if count <= 0 {
        return Ok(WireSlot::Empty);
    }
    let item_id = r.varint().map_err(|_| ())?;
    let (patch, charged) = read_patch(r, ids)?;
    Ok(WireSlot::Stack(WireStack {
        count,
        item_id,
        patch,
        charged,
    }))
}

/// How deep a `charged_projectiles` chain may nest before the walk gives up.
///
/// A projectile is itself an `ItemStackTemplate` with its own patch, which
/// could in principle carry `charged_projectiles` again. Vanilla never does
/// that, but the wire is not vanilla — a hostile server could nest until the
/// stack overflowed. The walk is bounded and reports [`PatchOutcome::Unwalkable`]
/// at the limit, which is the same fail-closed answer an unknown codec gets.
const MAX_PATCH_DEPTH: u32 = 4;

/// `DataComponentPatch.STREAM_CODEC.decode`, walked to the end whenever every
/// entry's codec is known.
///
/// The swing result is accumulated as the walk proceeds rather than returned
/// from the middle of it: the entries after the swing component still have to
/// be consumed, or the reader would be left mid-patch while the caller believed
/// it was aligned.
fn read_patch(
    r: &mut PacketReader,
    ids: DataComponentIds,
) -> Result<(PatchOutcome, PatchCharged), ()> {
    read_patch_at(r, ids, 0)
}

fn read_patch_at(
    r: &mut PacketReader,
    ids: DataComponentIds,
    depth: u32,
) -> Result<(PatchOutcome, PatchCharged), ()> {
    let added = r.varint().map_err(|_| ())?;
    let removed = r.varint().map_err(|_| ())?;
    if added == 0 && removed == 0 {
        // DataComponentPatch.EMPTY
        return Ok((
            PatchOutcome::Walked(PatchSwing::Absent),
            PatchCharged::Absent,
        ));
    }
    // The decoder sizes its map with `min(added + removed, 65536)`; a nonsense
    // count here is a malformed body, not a huge patch.
    if !(0..=65536).contains(&added) || !(0..=65536).contains(&removed) {
        return Err(());
    }
    let mut swing = PatchSwing::Absent;
    let mut charged = PatchCharged::Absent;
    for _ in 0..added {
        let ty = r.varint().map_err(|_| ())?;
        if ty == ids.swing_animation {
            // `SwingAnimation.STREAM_CODEC` = composite(type idMapper, VarInt).
            let kind = SwingAnimationType::from_wire_id(r.varint().map_err(|_| ())?);
            let duration = r.varint().map_err(|_| ())?;
            swing = PatchSwing::Set(SwingAnimation::new(kind, duration));
            continue;
        }
        if ty == ids.damage {
            r.varint().map_err(|_| ())?; // ByteBufCodecs.VAR_INT
            continue;
        }
        if ty == ids.charged_projectiles {
            match read_projectile_list(r, ids, depth)? {
                Some(non_empty) => charged = PatchCharged::Set(non_empty),
                // A nested patch this decoder cannot walk. The reader is parked
                // mid-value, so the whole enclosing stack is unwalkable — the
                // charge answer is lost with it.
                None => return Ok((PatchOutcome::Unwalkable, PatchCharged::Absent)),
            }
            continue;
        }
        // An un-transcribed codec: the reader stops here, mid-value.
        return Ok((PatchOutcome::Unwalkable, PatchCharged::Absent));
    }
    for _ in 0..removed {
        let ty = r.varint().map_err(|_| ())?;
        // A component cannot be both set and removed in one patch (the patch is
        // a map), so neither of these can contradict an earlier `Set`.
        if ty == ids.swing_animation {
            swing = PatchSwing::Removed;
        }
        if ty == ids.charged_projectiles {
            charged = PatchCharged::Removed;
        }
    }
    Ok((PatchOutcome::Walked(swing), charged))
}

/// `ByteBufCodecs.list(1024)` of `ItemStackTemplate.STREAM_CODEC`, which is
/// `composite(Item.STREAM_CODEC, VAR_INT count, DataComponentPatch.STREAM_CODEC)`.
///
/// Returns `Some(list is non-empty)` when the whole list was walked, or `None`
/// when a nested patch could not be — in which case the reader is parked.
///
/// Note this is `ItemStackTemplate`, **not** `ItemStack`: there is no
/// leading optional-count, and the count is a plain VarInt in the middle.
/// Reading it as an optional stack would desynchronise the whole packet.
fn read_projectile_list(
    r: &mut PacketReader,
    ids: DataComponentIds,
    depth: u32,
) -> Result<Option<bool>, ()> {
    if depth >= MAX_PATCH_DEPTH {
        return Ok(None);
    }
    let len = r.varint().map_err(|_| ())?;
    // `ChargedProjectiles` rejects more than 1024 items and the codec's list is
    // size-limited to the same, so anything else is a malformed body.
    if !(0..=1024).contains(&len) {
        return Err(());
    }
    for _ in 0..len {
        r.varint().map_err(|_| ())?; // Item.STREAM_CODEC — raw registry id
        r.varint().map_err(|_| ())?; // ByteBufCodecs.VAR_INT — count
        let (outcome, _) = read_patch_at(r, ids, depth + 1)?;
        if outcome == PatchOutcome::Unwalkable {
            return Ok(None);
        }
    }
    Ok(Some(len > 0))
}

/// The value `ItemStack.getSwingAnimation()` would return for a decoded stack,
/// or why it cannot be known.
///
/// - [`PatchSwing::Set`] — the patch wins outright, whatever the item is.
/// - [`PatchSwing::Removed`] — the component is absent from the patched map, so
///   `getOrDefault` yields `SwingAnimation.DEFAULT`, **not** the prototype.
/// - [`PatchSwing::Absent`] — the item's prototype value, if the item is
///   registered; an unregistered id is [`UnknownSwing::UnregisteredItem`].
/// - [`PatchOutcome::Unwalkable`] — [`UnknownSwing::UnwalkableComponent`]. No
///   fallback: an override could be hiding behind the component we could not
///   walk, and guessing the prototype would be a wrong visual presented as a
///   right one.
pub fn resolve_swing(stack: &WireStack, prototypes: &SwingAnimations) -> SwingResolution {
    match stack.patch {
        PatchOutcome::Unwalkable => SwingResolution::Unknown(UnknownSwing::UnwalkableComponent),
        PatchOutcome::Walked(PatchSwing::Set(v)) => SwingResolution::Exact(v),
        PatchOutcome::Walked(PatchSwing::Removed) => SwingResolution::Exact(SwingAnimation::DEFAULT),
        PatchOutcome::Walked(PatchSwing::Absent) => match prototypes.of(stack.item_id) {
            Some(v) => SwingResolution::Exact(v),
            None => SwingResolution::Unknown(UnknownSwing::UnregisteredItem),
        },
    }
}

/// The value `ItemStack.getUseDuration()` / `getUseAnimation()` would return
/// for a decoded stack (M23), or `None` when it cannot be known.
///
/// Simpler than [`resolve_swing`] because there is no patch arm: the three
/// components the base rule reads (`consumable`, `blocks_attacks`,
/// `kinetic_weapon`) are all ones this decoder cannot walk, so a stack that
/// patched any of them is already [`PatchOutcome::Unwalkable`] and never gets
/// here. What remains is the prototype, keyed by item id.
pub fn resolve_use(stack: &WireStack, profiles: &UseProfiles) -> Option<UseProfile> {
    match stack.patch {
        PatchOutcome::Unwalkable => None,
        PatchOutcome::Walked(_) => profiles.of(stack.item_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: DataComponentIds = DataComponentIds {
        swing_animation: 40,
        damage: 3,
        charged_projectiles: 7,
    };

    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut n = v as u32;
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    /// count + item + patch(added…, removed…), built independently of any
    /// writer under test.
    fn stack(count: i32, item: i32, added: &[(i32, Vec<u8>)], removed: &[i32]) -> Vec<u8> {
        let mut b = Vec::new();
        varint(count, &mut b);
        if count <= 0 {
            return b;
        }
        varint(item, &mut b);
        varint(added.len() as i32, &mut b);
        varint(removed.len() as i32, &mut b);
        for (ty, value) in added {
            varint(*ty, &mut b);
            b.extend_from_slice(value);
        }
        for ty in removed {
            varint(*ty, &mut b);
        }
        b
    }

    fn swing_value(kind: u8, duration: i32) -> Vec<u8> {
        let mut v = Vec::new();
        varint(kind as i32, &mut v);
        varint(duration, &mut v);
        v
    }

    fn read(bytes: &[u8]) -> Result<WireSlot, ()> {
        read_optional(&mut PacketReader::new(bytes), IDS)
    }

    fn patch_of(slot: WireSlot) -> PatchOutcome {
        match slot {
            WireSlot::Stack(s) => s.patch,
            WireSlot::Empty => panic!("expected a stack"),
        }
    }

    #[test]
    fn non_positive_count_is_the_empty_stack_and_reads_nothing_more() {
        for count in [0, -1, -128] {
            let mut b = Vec::new();
            varint(count, &mut b);
            assert_eq!(read(&b), Ok(WireSlot::Empty), "count {count}");
        }
    }

    #[test]
    fn a_plain_stack_has_an_empty_patch() {
        let s = read(&stack(1, 949, &[], &[])).unwrap();
        assert!(s.aligned());
        assert_eq!(patch_of(s), PatchOutcome::Walked(PatchSwing::Absent));
    }

    #[test]
    fn an_explicit_swing_animation_override_is_decoded() {
        let s = read(&stack(1, 100, &[(IDS.swing_animation, swing_value(2, 11))], &[])).unwrap();
        assert_eq!(
            patch_of(s),
            PatchOutcome::Walked(PatchSwing::Set(SwingAnimation::new(
                SwingAnimationType::Stab,
                11
            )))
        );
    }

    #[test]
    fn the_walk_continues_past_the_swing_component_and_stays_aligned() {
        // swing_animation FIRST, then a damage entry and a removal. Returning
        // as soon as the swing was found would leave 3 unread entries and
        // desynchronise the next slot — the exact bug this test pins.
        let mut dmg = Vec::new();
        varint(37, &mut dmg);
        let body = stack(
            1,
            100,
            &[(IDS.swing_animation, swing_value(2, 11)), (IDS.damage, dmg)],
            &[19],
        );
        let mut r = PacketReader::new(&body);
        let slot = read_optional(&mut r, IDS).unwrap();
        assert!(slot.aligned());
        assert_eq!(
            patch_of(slot),
            PatchOutcome::Walked(PatchSwing::Set(SwingAnimation::new(
                SwingAnimationType::Stab,
                11
            )))
        );
        assert!(r.u8().is_err(), "the whole patch was consumed");
    }

    #[test]
    fn damage_is_walked_past_to_reach_a_later_override() {
        let mut dmg = Vec::new();
        varint(37, &mut dmg);
        let s = read(&stack(
            1,
            100,
            &[(IDS.damage, dmg), (IDS.swing_animation, swing_value(0, 4))],
            &[],
        ))
        .unwrap();
        assert_eq!(
            patch_of(s),
            PatchOutcome::Walked(PatchSwing::Set(SwingAnimation::new(
                SwingAnimationType::None,
                4
            )))
        );
    }

    #[test]
    fn an_unknown_component_before_the_swing_stops_the_walk() {
        let s = read(&stack(
            1,
            100,
            &[(13, vec![0xAA, 0xBB, 0xCC])],
            &[IDS.swing_animation],
        ))
        .unwrap();
        assert_eq!(patch_of(s), PatchOutcome::Unwalkable);
        assert!(!s.aligned());
    }

    #[test]
    fn an_unknown_component_after_the_swing_still_stops_the_walk() {
        // The override *was* read, but the reader is now stuck: reporting
        // `Walked` here would desynchronise the packet, so the whole stack is
        // Unwalkable and its swing unknown.
        let body = stack(
            1,
            100,
            &[
                (IDS.swing_animation, swing_value(2, 11)),
                (13, vec![0xAA, 0xBB]),
            ],
            &[],
        );
        let s = read(&body).unwrap();
        assert_eq!(patch_of(s), PatchOutcome::Unwalkable);
        assert!(!s.aligned());
    }

    #[test]
    fn an_unresolved_patch_leaves_the_reader_mid_value() {
        // The three junk bytes after the un-transcribed component are NOT
        // consumed — which is exactly why the caller must stop rather than
        // read a second slot out of them.
        let bytes = stack(1, 100, &[(13, vec![0xAA, 0xBB, 0xCC])], &[]);
        let mut r = PacketReader::new(&bytes);
        let s = read_optional(&mut r, IDS).unwrap();
        assert!(!s.aligned());
        assert_eq!(r.u8().ok(), Some(0xAA), "the component's value is still unread");
    }

    #[test]
    fn a_removal_only_patch_is_fully_walkable() {
        let s = read(&stack(1, 100, &[], &[3, IDS.swing_animation, 19])).unwrap();
        assert_eq!(patch_of(s), PatchOutcome::Walked(PatchSwing::Removed));
        // …and a removal list without the swing component is Absent.
        let s = read(&stack(1, 100, &[], &[3, 19])).unwrap();
        assert_eq!(patch_of(s), PatchOutcome::Walked(PatchSwing::Absent));
    }

    #[test]
    fn a_truncated_stack_is_an_error_not_a_guess() {
        assert_eq!(read(&[]), Err(()));
        assert_eq!(read(&[1]), Err(())); // count but no item
        assert_eq!(read(&[1, 100]), Err(())); // item but no patch header
        assert_eq!(read(&[1, 100, 1, 0]), Err(())); // added=1 but no type
    }

    // `resolve_swing`'s prototype/unregistered arms need a *real* item registry
    // (a spear whose prototype differs from the default, and an id outside the
    // registry) to be worth asserting, so they are witnessed in
    // `rewo swingshot --check` rather than against a stand-in table here.
}
