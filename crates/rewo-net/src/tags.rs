//! `update_tags` (M69) — the server's own answer for what its datapack tags
//! contain.
//!
//! **Decode and model only.** Nothing here is wired into M19's
//! `ItemTags.SPEARS` lookup or M42's enchantment `curse` / `tooltip_order`
//! ordering; [`TagOverrides`] is the state a later wiring reads, and §"What
//! wiring this would take" below says exactly what stands between the two.
//! A half-applied override — one lookup honouring the server and its
//! neighbour still reading the jar — would be strictly worse than the current
//! honest divergence, because the two would disagree with each other as well
//! as with the server.
//!
//! ## Why this matters more than its size suggests
//!
//! Rewo reads vanilla's datapack tags out of the **client jar**:
//! `data/minecraft/tags/item/spears.json` decides M19's `SPEAR` arm pose, and
//! `data/minecraft/tags/enchantment/{curse,tooltip_order}.json` decide which
//! tooltip lines are red and in what order they appear. `handleUpdateTags` is
//! where the *server* says what its tags actually are, and it is applied for
//! every non-memory connection — i.e. every real one.
//!
//! So a server whose datapack retags one item produces, in Rewo, a wrong swing
//! duration or a missing tooltip line with **no error anywhere**: the ids still
//! round-trip, the strings are still real, and only someone who already knows
//! the right answer can see it. That is the M64 alphabetisation trap one layer
//! up, and it is why the packet ranked above a dozen more visible gaps in
//! `REWO_PACKET_COVERAGE.md` §3.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/network/protocol/common/ClientboundUpdateTagsPacket.java`
//! - `net/minecraft/tags/TagNetworkSerialization.java` — `NetworkPayload.read`
//! - `net/minecraft/network/FriendlyByteBuf.java` — `readMap`,
//!   `readRegistryKey`, `readIntIdList`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleUpdateTags`, `updateTags`
//! - `net/minecraft/core/MappedRegistry.java` — `prepareTagReload` and the
//!   `PendingTags.apply` that rule 1 below comes from
//!
//! ## The wire
//!
//! ```text
//! VarInt                       registry count
//!   Identifier                   registry key      (e.g. minecraft:item)
//!   VarInt                       tag count
//!     Identifier                   tag name        (e.g. minecraft:spears)
//!     VarInt                       id count
//!       VarInt × n                   NUMERIC registry ids
//! ```
//!
//! Two levels of `readMap` and one `readIntIdList`, nested. **The leaves are
//! numeric registry ids, not names** — resolving one back to a name needs the
//! registry, which for `minecraft:enchantment` is itself the server's
//! (M42's rule: a datapack registry's contents *and* its id order are the
//! server's). That asymmetry is the reason the two consumers are not equally
//! cheap to wire; see below.
//!
//! ## Three rules where the plausible implementation is silently wrong
//!
//! Each has a witness, and each has a mutation partner.
//!
//! 1. **A registry that appears is REPLACED WHOLE, not merged.**
//!    `PendingTags.apply` ends `allTags = TagSet.fromMap(pendingTags)` — the
//!    map built from *this packet's* entries alone. A tag the packet omits for
//!    a registry the packet mentions does not survive; it becomes absent, and
//!    `stack.is(thatTag)` is then false. Merging instead would keep a tag the
//!    server has deleted, which is the failure that looks most like working.
//! 2. **A registry the packet omits ENTIRELY is untouched.** `handleUpdateTags`
//!    iterates only `packet.getTags()`, so a registry absent from the map keeps
//!    whatever it had. Rules 1 and 2 are the same code read at two scopes, and
//!    a model that clears everything before applying gets rule 1 right and rule
//!    2 backwards.
//! 3. **An id outside the registry is DROPPED, not an error.**
//!    `deserializeTagsFromNetwork` is
//!    `ids.intStream().mapToObj(registry::get).flatMap(Optional::stream)` —
//!    `Registry.get(int)` returns `Optional.empty()` for an unknown id and
//!    `flatMap(Optional::stream)` discards it. So an id the client does not
//!    know shrinks the tag silently. That resolution needs a registry, so it
//!    is **not** performed here: [`TagOverrides`] stores the ids as sent and
//!    [`TagOverrides::contains`] is a membership test over them, which gives
//!    the same answer for every id the client does know.
//!
//! ## Every registry is kept, and that is a decision
//!
//! A vanilla server sends tags for every network-synchronised registry —
//! block, item, fluid, entity_type, enchantment, and a dozen more — and Rewo
//! models a handful. The alternative was to keep an allow-list and discard the
//! rest. Three reasons it keeps everything instead:
//!
//! * **Discarding is not cheaper.** There is no length prefix on a registry's
//!   payload, so skipping one still means walking every identifier and every
//!   id in it. The only saving would be the allocation.
//! * **An allow-list is a second place to get a registry name wrong**, and a
//!   name typo there fails exactly the way this whole milestone exists to
//!   prevent: silently, with the jar's answer standing in.
//! * **Vanilla keeps them all**, and the set Rewo will want is not final.
//!
//! An unknown registry is therefore data, not an error. Vanilla's own stance
//! is stricter — `updateTags` calls `registryAccess.lookupOrThrow(key)`, so an
//! unrecognised registry key drops the connection — but Rewo does not hold the
//! client-side registry set that judgement needs, and inventing one would
//! reject registries that are merely unmodelled.
//!
//! ## What wiring this would take (deliberately not done here)
//!
//! * **M19 / `ItemTags.SPEARS`** is the near half. The *values* already line
//!   up: `rewo_data::item_tags::ItemTag` is keyed by protocol id and so is
//!   this packet, so the override is `ItemTag::from_ids(overrides.tag(
//!   ITEM_REGISTRY, SPEARS_TAG)?)` and nothing more. What is missing is the
//!   *plumbing*:
//!   `spears` is an owned field on three structs in `live_cmd.rs` and passed
//!   by reference through several more, all built before the session exists,
//!   and the override arrives inside the net session. Threading it is
//!   mechanical but it is eight call sites and a lifetime, and it cannot be
//!   graded by any existing gate — `swingshot` builds its own `ItemTag` from
//!   the jar and never opens a connection.
//! * **M42 / the enchantment tags** is the far half and is **blocked**, not
//!   merely unplumbed. `rewo_data::enchantments` stores `curse` and
//!   `tooltip_order` as `Vec<String>` of enchantment *names*, and this packet
//!   carries *ids*. Bridging them needs `PlaySession::enchantments` — the
//!   registry parsed during configuration, whose index is the protocol id — to
//!   be read at the same moment, and the two arrive in either order (a
//!   datapack reload re-sends `update_tags` in play, long after
//!   `registry_data`). That is a real ordering problem with a real answer, and
//!   it is not one to decide in the same change that first decodes the packet.
//!
//! [`TagOverrides::contains`] is the seam both would use, and it returns
//! `Option<bool>` precisely so a caller can tell "the server has not spoken
//! about this tag" (fall back to the jar) from "the server says no".

use std::collections::HashMap;

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `minecraft:item` — the registry `ItemTags.SPEARS` lives in (M19).
pub const ITEM_REGISTRY: &str = "minecraft:item";
/// `minecraft:enchantment` — `EnchantmentTags.CURSE` and `TOOLTIP_ORDER` (M42).
pub const ENCHANTMENT_REGISTRY: &str = "minecraft:enchantment";
/// `ItemTags.SPEARS`.
pub const SPEARS_TAG: &str = "minecraft:spears";
/// `EnchantmentTags.CURSE`.
pub const CURSE_TAG: &str = "minecraft:curse";
/// `EnchantmentTags.TOOLTIP_ORDER`.
pub const TOOLTIP_ORDER_TAG: &str = "minecraft:tooltip_order";

/// One registry's tag set, exactly as the packet carried it.
///
/// A `Vec` rather than a map because **order is load-bearing for at least one
/// consumer**: `EnchantmentTags.TOOLTIP_ORDER` is a tag whose *sequence* is
/// the tooltip's sequence (M42), and `readMap` preserves the wire order it was
/// written in. A `HashMap` here would destroy it, and the loss would show as
/// tooltip lines in an arbitrary order — plausible enough to go unnoticed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryTags {
    /// `minecraft:item`, `minecraft:enchantment`, …
    pub registry: String,
    /// `(tag name, member ids)` in wire order. Ids are this registry's
    /// **protocol ids**, unresolved — see rule 3.
    pub tags: Vec<(String, Vec<i32>)>,
}

impl RegistryTags {
    /// One tag's members, or `None` if this registry's payload did not carry
    /// it. Distinct from `Some(&[])`, which is a tag the server declared and
    /// declared empty.
    pub fn tag(&self, name: &str) -> Option<&[i32]> {
        self.tags
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, ids)| ids.as_slice())
    }
}

/// One decoded `update_tags` packet: every registry it mentioned, in wire
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagUpdate {
    pub registries: Vec<RegistryTags>,
}

impl TagUpdate {
    pub fn is_empty(&self) -> bool {
        self.registries.is_empty()
    }

    /// Total tags across every registry — what the logs report, and what a
    /// witness counts to prove nothing was dropped mid-walk.
    pub fn tag_count(&self) -> usize {
        self.registries.iter().map(|r| r.tags.len()).sum()
    }
}

/// The applied state: what the server has most recently said about each
/// registry it has spoken about.
///
/// Keyed by registry name. A registry absent from this map is one the server
/// has never mentioned, and the jar's answer stands for it — rule 2.
#[derive(Clone, Debug, Default)]
pub struct TagOverrides {
    by_registry: HashMap<String, RegistryTags>,
    /// How many `update_tags` packets have been applied. A datapack reload
    /// sends a second one mid-play, and a consumer that caches a resolved set
    /// needs to know it went stale.
    pub generation: u32,
}

impl TagOverrides {
    /// Apply one packet: **replace** each mentioned registry whole, leave
    /// every other registry alone. Rules 1 and 2.
    pub fn apply(&mut self, update: &TagUpdate) {
        for reg in &update.registries {
            self.by_registry
                .insert(reg.registry.clone(), reg.clone());
        }
        self.generation = self.generation.wrapping_add(1);
    }

    /// Whether the server has said anything at all yet.
    pub fn is_empty(&self) -> bool {
        self.by_registry.is_empty()
    }

    pub fn registry_count(&self) -> usize {
        self.by_registry.len()
    }

    /// One registry's whole tag set, if the server has sent it.
    pub fn registry(&self, registry: &str) -> Option<&RegistryTags> {
        self.by_registry.get(registry)
    }

    /// One tag's member ids.
    ///
    /// `None` means the server has not spoken about this tag — either the
    /// registry never appeared (rule 2) or the registry appeared without it
    /// (rule 1, where the tag has been *deleted*). Those two are the same
    /// answer for a consumer that falls back to the jar and different for one
    /// that does not, which is why the distinction stays available through
    /// [`Self::registry`] rather than being collapsed here.
    pub fn tag(&self, registry: &str, tag: &str) -> Option<&[i32]> {
        self.by_registry.get(registry)?.tag(tag)
    }

    /// `stack.is(tag)` over the server's answer.
    ///
    /// `None` = the server has not spoken; the caller falls back to the jar.
    /// `Some(false)` = the server has spoken and this id is not a member.
    /// Those must not be conflated: the first is ignorance and the second is
    /// an answer, and a `bool` return would turn every unsent tag into "no".
    pub fn contains(&self, registry: &str, tag: &str, id: i32) -> Option<bool> {
        Some(self.tag(registry, tag)?.contains(&id))
    }

    /// Whether this registry's set was replaced by the last applied packet —
    /// i.e. whether rule 1 has fired for it at least once.
    pub fn mentions(&self, registry: &str) -> bool {
        self.by_registry.contains_key(registry)
    }
}

/// The largest registry count and per-registry tag count a body may declare.
///
/// `PacketReader::count` already bounds a count by the bytes left, which is
/// the real defence; these are a second, cheaper bound so a body that is
/// *large* rather than *malformed* cannot make us allocate a vector per tag
/// before the byte bound bites. A vanilla 26.2 server sends on the order of 20
/// registries and a few hundred tags each.
const MAX_REGISTRIES: usize = 4096;
const MAX_TAGS_PER_REGISTRY: usize = 65_536;

/// Decode one `ClientboundUpdateTagsPacket` body.
///
/// Every byte of the body belongs to the nested maps, so a successful decode
/// leaves the reader exactly at the end — which is what the witnesses assert
/// against a sentinel.
///
/// A short or malformed body is an `Err` and the caller applies **nothing**.
/// Half-applying is the one genuinely bad option here: rule 1 makes an apply
/// destructive, so a partial packet would delete the tags of every registry it
/// managed to read past and leave the rest at the jar's answer — two different
/// sources of truth inside one client.
pub fn read_update_tags(body: &[u8]) -> Result<TagUpdate> {
    let mut r = PacketReader::new(body);
    // Each registry costs at least an identifier byte and a count byte.
    let count = r.count("update_tags registries", 2)?;
    let mut registries = Vec::with_capacity(count.min(MAX_REGISTRIES));
    for _ in 0..count {
        registries.push(read_registry_tags(&mut r)?);
    }
    Ok(TagUpdate { registries })
}

/// One `(ResourceKey<Registry>, NetworkPayload)` map entry.
///
/// `readRegistryKey` is `ResourceKey.createRegistryKey(readIdentifier())` — a
/// plain identifier on the wire, with no registry id and no prefix. There is
/// nothing to distinguish it from a tag name except its position, which is why
/// the nesting depth has to be exactly right: read one level too few and every
/// registry key is a tag name from then on, with no decode error to show for
/// it.
fn read_registry_tags(r: &mut PacketReader<'_>) -> Result<RegistryTags> {
    let registry = r.identifier()?;
    let tag_count = r.count("update_tags tags", 2)?;
    let mut tags = Vec::with_capacity(tag_count.min(MAX_TAGS_PER_REGISTRY));
    for _ in 0..tag_count {
        let name = r.identifier()?;
        // `readIntIdList` — VarInt count, then that many VarInts. Each id is
        // at least one byte.
        let id_count = r.count("update_tags ids", 1)?;
        let mut ids = Vec::with_capacity(id_count);
        for _ in 0..id_count {
            ids.push(r.varint()?);
        }
        tags.push((name, ids));
    }
    Ok(RegistryTags { registry, tags })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// A byte the decoder cannot consume: `read_update_tags` ends on a known
    /// field, so if the reader is not sitting exactly on this the walk read the
    /// wrong number of bytes. Every wire witness below asserts it.
    const SENTINEL: u8 = 0xA7;

    fn assert_consumed_exactly(r: &mut PacketReader<'_>) {
        assert_eq!(r.remaining(), 1, "decoder must stop on the sentinel byte");
        assert_eq!(r.u8().unwrap(), SENTINEL, "trailing byte is the sentinel");
        assert_eq!(r.remaining(), 0);
    }

    /// Encode a `TagUpdate` back onto the wire, so a witness can state the
    /// bytes as a structure and still grade the reader against them.
    fn encode(update: &TagUpdate) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.varint(update.registries.len() as i32);
        for reg in &update.registries {
            w.string(&reg.registry);
            w.varint(reg.tags.len() as i32);
            for (name, ids) in &reg.tags {
                w.string(name);
                w.varint(ids.len() as i32);
                for id in ids {
                    w.varint(*id);
                }
            }
        }
        w.into_bytes()
    }

    fn with_sentinel(body: &[u8]) -> Vec<u8> {
        let mut v = body.to_vec();
        v.push(SENTINEL);
        v
    }

    fn reg(registry: &str, tags: &[(&str, &[i32])]) -> RegistryTags {
        RegistryTags {
            registry: registry.into(),
            tags: tags
                .iter()
                .map(|(n, ids)| ((*n).to_string(), ids.to_vec()))
                .collect(),
        }
    }

    fn update(registries: &[RegistryTags]) -> TagUpdate {
        TagUpdate {
            registries: registries.to_vec(),
        }
    }

    /// A body shaped like a real one: two registries, several tags, ids that
    /// need one and two VarInt bytes.
    fn sample() -> TagUpdate {
        update(&[
            reg(
                ITEM_REGISTRY,
                &[
                    (SPEARS_TAG, &[7, 9, 300]),
                    ("minecraft:swords", &[1, 2, 3, 4, 5]),
                    ("minecraft:empty_by_design", &[]),
                ],
            ),
            reg(
                ENCHANTMENT_REGISTRY,
                &[(CURSE_TAG, &[10, 11]), (TOOLTIP_ORDER_TAG, &[3, 1, 2])],
            ),
        ])
    }

    // -- the wire --------------------------------------------------------

    #[test]
    fn a_nested_map_round_trips_and_consumes_exactly_its_body() {
        let want = sample();
        let bytes = with_sentinel(&encode(&want));
        let mut r = PacketReader::new(&bytes);
        // Decode through the production reader over the same buffer, so the
        // sentinel check measures the real walk.
        let count = r.count("registries", 2).unwrap();
        let mut got = TagUpdate::default();
        for _ in 0..count {
            got.registries.push(read_registry_tags(&mut r).unwrap());
        }
        assert_eq!(got, want);
        assert_consumed_exactly(&mut r);
        // And the top-level entry point agrees on the same bytes.
        assert_eq!(read_update_tags(&bytes[..bytes.len() - 1]).unwrap(), want);
    }

    /// **The nesting-depth witness.** The registry key and a tag name are both
    /// bare identifiers, so a reader that is one level off still parses
    /// *something*. Mutation partner: delete the `tag_count` read in
    /// `read_registry_tags` and treat the identifier that follows as the first
    /// tag — the body still decodes for a one-registry one-tag case and this
    /// fails, because the counts no longer line up over two registries.
    #[test]
    fn the_registry_key_and_the_tag_name_are_told_apart_only_by_position() {
        let want = sample();
        let bytes = encode(&want);
        let got = read_update_tags(&bytes).unwrap();
        assert_eq!(got.registries.len(), 2);
        assert_eq!(got.registries[0].registry, ITEM_REGISTRY);
        assert_eq!(got.registries[1].registry, ENCHANTMENT_REGISTRY);
        assert_eq!(got.tag_count(), 5);
        // Nothing named a registry ended up as a tag and vice versa.
        for r in &got.registries {
            for (name, _) in &r.tags {
                assert_ne!(name, ITEM_REGISTRY);
                assert_ne!(name, ENCHANTMENT_REGISTRY);
            }
        }
    }

    /// The leaves are numeric ids. A reader that took them as identifiers, or
    /// as zig-zag, would produce a different set — 300 is the two-byte case
    /// that separates a VarInt from a plain byte, and `-1` is the five-byte
    /// two's-complement case that separates it from zig-zag.
    #[test]
    fn the_leaves_are_twos_complement_var_int_ids() {
        let want = update(&[reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[0, 127, 128, 300, -1])])]);
        let bytes = with_sentinel(&encode(&want));
        let mut r = PacketReader::new(&bytes[..]);
        let n = r.count("registries", 2).unwrap();
        assert_eq!(n, 1);
        let got = read_registry_tags(&mut r).unwrap();
        assert_eq!(got.tag(SPEARS_TAG).unwrap(), &[0, 127, 128, 300, -1]);
        assert_consumed_exactly(&mut r);
    }

    /// An empty tag list and an absent tag are different states, and the
    /// decode has to preserve both. Mutation partner: filter empty `ids` out
    /// in `read_registry_tags` — `tag()` then answers `None` for a tag the
    /// server explicitly emptied, which reads as "fall back to the jar" and
    /// restores the very members the server deleted.
    #[test]
    fn an_empty_tag_is_kept_and_is_not_an_absent_tag() {
        let bytes = encode(&update(&[reg(
            ITEM_REGISTRY,
            &[(SPEARS_TAG, &[]), ("minecraft:swords", &[4])],
        )]));
        let got = read_update_tags(&bytes).unwrap();
        let items = &got.registries[0];
        assert_eq!(items.tag(SPEARS_TAG), Some(&[][..]), "declared, empty");
        assert_eq!(items.tag("minecraft:axes"), None, "never declared");
        assert_ne!(items.tag(SPEARS_TAG), items.tag("minecraft:axes"));
    }

    /// A zero-registry packet is legal and means "nothing changed", not a
    /// malformed body.
    #[test]
    fn a_zero_registry_body_decodes_to_an_empty_update() {
        let bytes = with_sentinel(&encode(&TagUpdate::default()));
        assert_eq!(bytes.len(), 2, "one count byte plus the sentinel");
        let mut r = PacketReader::new(&bytes);
        let n = r.count("registries", 2).unwrap();
        assert_eq!(n, 0);
        assert_consumed_exactly(&mut r);
        assert!(read_update_tags(&bytes[..1]).unwrap().is_empty());
    }

    /// Every truncation of a real body is an error, never a partial decode.
    /// Rule 1 makes an apply destructive, so a body that decoded "as far as it
    /// got" would delete tags on the strength of bytes that were never sent.
    #[test]
    fn every_truncation_fails_rather_than_decoding_partially() {
        let bytes = encode(&sample());
        for cut in 0..bytes.len() {
            assert!(
                read_update_tags(&bytes[..cut]).is_err(),
                "a {cut}-byte prefix of a {}-byte body decoded",
                bytes.len()
            );
        }
        assert!(read_update_tags(&bytes).is_ok());
    }

    /// Trailing bytes are not consumed by the walk, which is the property the
    /// sentinel asserts stated as its own witness: the decoder stops where the
    /// structure ends and does not swallow whatever follows.
    #[test]
    fn a_body_with_trailing_bytes_leaves_them_unread() {
        let bytes = with_sentinel(&encode(&sample()));
        let mut r = PacketReader::new(&bytes);
        let n = r.count("registries", 2).unwrap();
        for _ in 0..n {
            read_registry_tags(&mut r).unwrap();
        }
        assert_consumed_exactly(&mut r);
    }

    // -- the model -------------------------------------------------------

    /// **Rule 1.** A registry that appears is replaced whole. Mutation
    /// partner: merge the incoming tags into the stored ones instead of
    /// inserting — `minecraft:swords` survives, and the client keeps honouring
    /// a tag the server deleted.
    #[test]
    fn a_mentioned_registry_is_replaced_whole_not_merged() {
        let mut o = TagOverrides::default();
        o.apply(&update(&[reg(
            ITEM_REGISTRY,
            &[(SPEARS_TAG, &[7]), ("minecraft:swords", &[9])],
        )]));
        assert_eq!(o.tag(ITEM_REGISTRY, "minecraft:swords"), Some(&[9][..]));

        o.apply(&update(&[reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[7, 8])])]));
        assert_eq!(o.tag(ITEM_REGISTRY, SPEARS_TAG), Some(&[7, 8][..]));
        assert_eq!(
            o.tag(ITEM_REGISTRY, "minecraft:swords"),
            None,
            "a tag the second packet omitted must not survive it"
        );
    }

    /// **Rule 2.** A registry the packet never mentions is untouched.
    /// Mutation partner: clear `by_registry` at the top of `apply` — this
    /// fails, and rule 1's witness still passes, which is why both exist.
    #[test]
    fn an_unmentioned_registry_survives_a_later_packet() {
        let mut o = TagOverrides::default();
        o.apply(&update(&[
            reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[7])]),
            reg(ENCHANTMENT_REGISTRY, &[(CURSE_TAG, &[1])]),
        ]));
        o.apply(&update(&[reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[7, 8])])]));
        assert_eq!(
            o.tag(ENCHANTMENT_REGISTRY, CURSE_TAG),
            Some(&[1][..]),
            "the enchantment registry was not in the second packet"
        );
        assert_eq!(o.tag(ITEM_REGISTRY, SPEARS_TAG), Some(&[7, 8][..]));
    }

    /// The three-way answer. A `bool` return would make every tag the server
    /// has not sent read as "not a member", which for `SPEARS` means every
    /// spear poses `ArmPose::Item` the moment the client talks to a server
    /// that omits the item registry.
    #[test]
    fn contains_separates_silence_from_a_negative_answer() {
        let mut o = TagOverrides::default();
        assert_eq!(o.contains(ITEM_REGISTRY, SPEARS_TAG, 7), None, "no packet yet");
        o.apply(&update(&[reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[7])])]));
        assert_eq!(o.contains(ITEM_REGISTRY, SPEARS_TAG, 7), Some(true));
        assert_eq!(o.contains(ITEM_REGISTRY, SPEARS_TAG, 9), Some(false));
        assert_eq!(
            o.contains(ENCHANTMENT_REGISTRY, CURSE_TAG, 7),
            None,
            "a registry the server never mentioned is silence, not a no"
        );
        assert_eq!(
            o.contains(ITEM_REGISTRY, "minecraft:axes", 7),
            None,
            "a tag this registry's payload omitted is silence too"
        );
    }

    /// `tooltip_order`'s *sequence* is the tooltip's sequence, so the decode
    /// and the store must both preserve wire order. Mutation partner: make
    /// `RegistryTags::tags` a `HashMap` or sort it — this fails, and no other
    /// witness in the module does.
    #[test]
    fn tag_order_and_id_order_both_survive_the_round_trip() {
        let want = update(&[reg(
            ENCHANTMENT_REGISTRY,
            &[
                (TOOLTIP_ORDER_TAG, &[9, 3, 7, 1]),
                (CURSE_TAG, &[4]),
                ("minecraft:zzz_last", &[0]),
            ],
        )]);
        let got = read_update_tags(&encode(&want)).unwrap();
        let names: Vec<&str> = got.registries[0]
            .tags
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(names, [TOOLTIP_ORDER_TAG, CURSE_TAG, "minecraft:zzz_last"]);
        assert_eq!(
            got.registries[0].tag(TOOLTIP_ORDER_TAG).unwrap(),
            &[9, 3, 7, 1],
            "an id list is an ordered sequence, not a set"
        );

        let mut o = TagOverrides::default();
        o.apply(&got);
        assert_eq!(
            o.tag(ENCHANTMENT_REGISTRY, TOOLTIP_ORDER_TAG).unwrap(),
            &[9, 3, 7, 1]
        );
    }

    /// A registry Rewo does not model is kept, not skipped — the decision the
    /// module header argues for, asserted so a later "optimisation" that adds
    /// an allow-list has to break a test to land.
    #[test]
    fn an_unmodelled_registry_is_kept_rather_than_discarded() {
        let mut o = TagOverrides::default();
        o.apply(&read_update_tags(&encode(&update(&[
            reg("minecraft:banner_pattern", &[("minecraft:no_item_required", &[0, 1])]),
            reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[7])]),
        ]))).unwrap());
        assert_eq!(o.registry_count(), 2);
        assert_eq!(
            o.tag("minecraft:banner_pattern", "minecraft:no_item_required"),
            Some(&[0, 1][..])
        );
        assert!(o.mentions("minecraft:banner_pattern"));
        assert!(!o.mentions("minecraft:fluid"));
    }

    /// A reload mid-session is a second packet, and a consumer caching a
    /// resolved set has to be able to see that it happened.
    #[test]
    fn the_generation_advances_once_per_applied_packet() {
        let mut o = TagOverrides::default();
        assert_eq!(o.generation, 0);
        assert!(o.is_empty());
        o.apply(&update(&[reg(ITEM_REGISTRY, &[(SPEARS_TAG, &[7])])]));
        assert_eq!(o.generation, 1);
        assert!(!o.is_empty());
        // Even a packet that changes nothing observable is a reload.
        o.apply(&TagUpdate::default());
        assert_eq!(o.generation, 2);
        assert_eq!(o.tag(ITEM_REGISTRY, SPEARS_TAG), Some(&[7][..]));
    }
}
