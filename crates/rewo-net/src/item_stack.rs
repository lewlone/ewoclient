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

pub use crate::component_wire::{ContainerSlot, ItemTemplate};
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
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Clone, Debug, PartialEq)]
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
    /// Whether the patch carried **any** entry, added or removed (M35).
    ///
    /// Not what any of them were — that needs the per-type codecs, and only a
    /// handful are transcribed. This is the one bit the container click
    /// arithmetic can honestly use: `ItemStack.isSameItemSameComponents` must
    /// be false whenever either side carries components Rewo cannot compare,
    /// so two patched stacks swap rather than merge. See
    /// [`rewo_world::inventory::ItemSlot::has_components`].
    pub patched: bool,
    /// What the patch said, for the components the client reads (M41).
    pub components: StackComponents,
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
    let (patch, charged, patched, components) = read_patch(r, ids)?;
    Ok(WireSlot::Stack(WireStack {
        count,
        item_id,
        patch,
        charged,
        patched,
        components,
    }))
}

/// What one stack's patch said, for the components the client reads (M41).
///
/// Built during the walk rather than looked up afterwards, because the walk is
/// the only place the values exist: the patch is a delta, and a component it
/// does not mention is answered by the item's prototype, not by this.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StackComponents {
    /// `minecraft:damage`, the value a durability bar is drawn from.
    pub damage: Option<i32>,
    /// `minecraft:max_damage`. Usually absent — the prototype carries it — so
    /// a bar needs the item table as well as the patch.
    pub max_damage: Option<i32>,
    /// `minecraft:custom_name`, the anvil-given name, reduced to plain text.
    pub custom_name: Option<String>,
    /// `minecraft:item_name`, a *default* name an item may carry. Lower
    /// precedence than `custom_name` and higher than the translated id.
    pub item_name: Option<String>,
    pub lore: Vec<String>,
    /// `minecraft:rarity`'s id — the name's colour.
    pub rarity: Option<i32>,
    /// `(enchantment registry id, level)`, from **both**
    /// `minecraft:enchantments` and `minecraft:stored_enchantments`, because a
    /// tooltip lists an enchanted book's stored ones the same way.
    pub enchantments: Vec<(i32, i32)>,
    /// `ItemStack.isEnchanted()`, which is
    /// `!getOrDefault(ENCHANTMENTS, EMPTY).isEmpty()` — `minecraft:enchantments`
    /// **only**.
    ///
    /// Separate from the list above precisely because the list is the union of
    /// two components and this one is not. An enchanted book carries
    /// `stored_enchantments` and is not `isEnchanted()`, so it stays RARE
    /// where the merged list would promote it to EPIC (M50).
    pub is_enchanted: bool,
    pub unbreakable: bool,
    /// `minecraft:enchantment_glint_override` (M43). `None` is absent, which
    /// is *not* the same as `Some(false)`: absent defers to whether the stack
    /// is enchanted, false suppresses the glint on one that is.
    pub glint_override: Option<bool>,
    /// `minecraft:dyed_color`'s RGB (M47), the tint a dyeable armour layer
    /// takes. Absent is **not** "black": `DyedItemColor.getOrDefault` returns
    /// 0 for an absent component, and 0 is the value `getColorForLayer` reads
    /// as "this stack is undyed" — which sends a dyeable layer to its
    /// `color_when_undyed` instead.
    pub dyed_color: Option<i32>,
    /// `minecraft:trim`'s `(material, pattern)` registry ids (M48).
    ///
    /// Captured only when **both** holders are registry references. `holder`
    /// writes `id + 1` with 0 meaning an inline definition follows, and an
    /// inline `TrimMaterial` is a chat component and an asset map — decodable,
    /// but never sent by a server whose registries the client already has.
    /// An inline one leaves this `None` and falls through to the generic walk,
    /// which consumes it correctly.
    pub trim: Option<(i32, i32)>,
    /// `minecraft:bundle_contents`' stacks (M61), in wire order.
    ///
    /// **`None` and `Some(vec![])` are different answers.** `None` is a patch
    /// that did not mention the component, so `getOrDefault` hands back
    /// `BundleContents.EMPTY` — which for a bundle means an empty grid and for
    /// anything else means it is not a bundle at all, and only the item id
    /// distinguishes those. `Some(vec![])` is a patch that set the component
    /// to an empty list, which vanilla renders as the empty-bundle blurb
    /// rather than as no tooltip image. A removal is in
    /// [`Self::removed`] and resolves through the default, like `None`.
    ///
    /// Every element is a whole `ItemStackTemplate`, so the grid's counts and
    /// the icons' item ids both come from here. What is *not* here is each
    /// element's own components: see [`ItemTemplate::patched`] for why, and
    /// for which vanilla behaviour that leaves out of reach.
    pub bundle: Option<Vec<ItemTemplate>>,
    /// `minecraft:container`'s slots (M63), in wire order, `None` where the
    /// slot is empty.
    ///
    /// `None` and `Some(vec![])` differ here exactly as they do for
    /// [`Self::bundle`]: a patch that never mentioned the component resolves
    /// through `ItemContainerContents.EMPTY`, and only the item id says
    /// whether that means "an empty shulker box" or "not a container at all".
    ///
    /// The **outer** option is the slot's — `ItemContainerContents.items` is a
    /// `List<Optional<ItemStackTemplate>>` indexed by slot number, so a gap is
    /// a real position and not a shorter list. `addToTooltip` skips the gaps
    /// (`nonEmptyItemsStream`); anything drawing a grid needs them.
    pub container: Option<Vec<Option<ContainerSlot>>>,
    /// Component ids the patch **removed**. A removal is not the same as an
    /// absence: `getOrDefault` then answers with the type's default rather
    /// than the item's prototype value.
    pub removed: Vec<i32>,
    /// A canonical digest of every entry, added or removed, interpreted or
    /// not.
    ///
    /// This is what makes `ItemStack.isSameItemSameComponents` **exact** where
    /// M35 could only ask "does either side carry components at all". Built
    /// from the (type id, raw value bytes) pairs **sorted by type id**, so two
    /// patches that encode the same entries in a different order still agree —
    /// the patch is written from a map, and map order is not part of its
    /// meaning.
    pub fingerprint: u64,
}

impl StackComponents {
    /// `stack.isDamaged()` — a damage value above zero.
    pub fn is_damaged(&self) -> bool {
        self.damage.unwrap_or(0) > 0
    }

    /// `ItemStack.hasFoil()` — whether this stack draws an enchantment glint.
    ///
    /// ```java
    /// Boolean override = get(ENCHANTMENT_GLINT_OVERRIDE);
    /// return override != null ? override : getItem().isFoil(this);
    /// ```
    ///
    /// and `Item.isFoil` is `stack.isEnchanted()`. So the override wins **both
    /// ways**: `true` puts a glint on a golden apple, `false` takes it off a
    /// Sharpness V sword. Reading the glint straight off the enchantment list
    /// gets the common case right and both of those wrong.
    pub fn has_foil(&self) -> bool {
        self.glint_override
            .unwrap_or(!self.enchantments.is_empty())
    }

    /// The stacks `BundleContents` would resolve to for this patch (M61) —
    /// what `BundleItem.getTooltipImage` reads to build its grid.
    ///
    /// `None` is `BundleContents.EMPTY`, which the wire never distinguishes
    /// from "this item is not a bundle": both a diamond and an empty bundle
    /// arrive with no `bundle_contents` entry, and it is the *item id* that
    /// tells them apart. So a caller drawing a grid must gate on the item
    /// being a bundle first and read this second — reading this alone would
    /// draw no grid for an empty bundle and be right by accident, then draw
    /// none for a full one the day the component moved.
    ///
    /// A removal (`!bundle_contents`) resolves through the default here, the
    /// same as an absence, because `getOrDefault` answers a removed component
    /// with the type's default rather than the item's prototype value.
    pub fn bundle_contents(&self) -> Option<&[ItemTemplate]> {
        self.bundle.as_deref()
    }

    /// The slots `ItemContainerContents` would resolve to for this patch (M63),
    /// gaps included.
    ///
    /// Carries the same "absent is indistinguishable from empty" caveat
    /// [`Self::bundle_contents`] documents, and for the same reason: a
    /// shulker box and a stone block both arrive with no `container` entry.
    pub fn container_contents(&self) -> Option<&[Option<ContainerSlot>]> {
        self.container.as_deref()
    }

    /// The occupied slots, in order — `ItemContainerContents.nonEmptyItems()`.
    ///
    /// What `addToTooltip` walks: it counts these, renders
    /// `item.container.item_count` for the first **five**, and if any remain
    /// adds one italic `item.container.more_items` for the rest. Empty slots
    /// never reach either line, which is why the filter belongs here rather
    /// than in the decode.
    pub fn container_items(&self) -> impl Iterator<Item = &ContainerSlot> {
        self.container
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(Option::as_ref)
    }

    /// The name a tooltip shows, given the item's translated display name.
    ///
    /// `ItemStack.getHoverName` is `getOrDefault(CUSTOM_NAME, getItemName())`,
    /// and `getItemName` is `getOrDefault(ITEM_NAME, item.getName())` — so the
    /// two components are a two-level override rather than alternatives.
    pub fn hover_name<'a>(&'a self, fallback: &'a str) -> &'a str {
        self.custom_name
            .as_deref()
            .or(self.item_name.as_deref())
            .unwrap_or(fallback)
    }
}

/// A 64-bit FNV-1a digest, used only to compare patches with each other.
///
/// Not a cryptographic hash and not a wire value: a collision would merge two
/// stacks vanilla keeps apart, which is the error direction M35's
/// approximation was written to avoid — but at 64 bits over the handful of
/// stacks in one inventory the probability is far below that of the
/// approximation it replaces.
fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
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
) -> Result<(PatchOutcome, PatchCharged, bool, StackComponents), ()> {
    read_patch_at(r, ids, 0)
}

fn read_patch_at(
    r: &mut PacketReader,
    ids: DataComponentIds,
    depth: u32,
) -> Result<(PatchOutcome, PatchCharged, bool, StackComponents), ()> {
    let added = r.varint().map_err(|_| ())?;
    let removed = r.varint().map_err(|_| ())?;
    if added == 0 && removed == 0 {
        // DataComponentPatch.EMPTY
        return Ok((
            PatchOutcome::Walked(PatchSwing::Absent),
            PatchCharged::Absent,
            false,
            StackComponents::default(),
        ));
    }
    // The decoder sizes its map with `min(added + removed, 65536)`; a nonsense
    // count here is a malformed body, not a huge patch.
    if !(0..=65536).contains(&added) || !(0..=65536).contains(&removed) {
        return Err(());
    }
    let mut swing = PatchSwing::Absent;
    let mut charged = PatchCharged::Absent;
    let mut comps = StackComponents::default();
    // Sorted by type id so the digest does not depend on the order the server
    // happened to iterate its map in.
    let mut digest: Vec<(i32, u64)> = Vec::new();
    for _ in 0..added {
        let ty = r.varint().map_err(|_| ())?;
        // The value's bytes, for the fingerprint. Taken as a span rather than
        // copied: the walk moves the reader past exactly this value, so the
        // two offsets bracket it whatever its codec was.
        let from = r.offset();
        // Every component this decoder interprets is *also* in the shape
        // table, so the table decides walkability and the match below decides
        // meaning. Keeping them separate is what stops a new interpretation
        // from silently becoming the only thing keeping a codec walkable.
        let Some(shape) = crate::component_wire::shape_for_id(ty) else {
            // An un-transcribed codec: the reader stops here, mid-value.
            crate::component_wire::report_unwalkable(ty);
            return Ok((PatchOutcome::Unwalkable, PatchCharged::Absent, true, comps));
        };
        if ty == ids.swing_animation {
            // `SwingAnimation.STREAM_CODEC` = composite(type idMapper, VarInt).
            let kind = SwingAnimationType::from_wire_id(r.varint().map_err(|_| ())?);
            let duration = r.varint().map_err(|_| ())?;
            swing = PatchSwing::Set(SwingAnimation::new(kind, duration));
        } else if ty == ids.charged_projectiles {
            match read_projectile_list(r, ids, depth)? {
                Some(non_empty) => charged = PatchCharged::Set(non_empty),
                // A nested patch this decoder cannot walk. The reader is parked
                // mid-value, so the whole enclosing stack is unwalkable — the
                // charge answer is lost with it.
                None => {
                    return Ok((PatchOutcome::Unwalkable, PatchCharged::Absent, true, comps))
                }
            }
        } else if !read_interpreted(r, ty, ids, shape, &mut comps)? {
            return Ok((PatchOutcome::Unwalkable, PatchCharged::Absent, true, comps));
        }
        let to = r.offset();
        digest.push((ty, fnv1a(0xcbf2_9ce4_8422_2325, r.bytes_between(from, to))));
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
        comps.removed.push(ty);
        // A removal has no value, so its own id is all there is to fold in —
        // and it must be folded in, or "damage removed" and "damage absent"
        // would fingerprint identically.
        digest.push((ty, 0));
    }
    digest.sort_unstable();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for (ty, v) in digest {
        h = fnv1a(h, &ty.to_le_bytes());
        h = fnv1a(h, &v.to_le_bytes());
    }
    comps.fingerprint = h;
    Ok((PatchOutcome::Walked(swing), charged, true, comps))
}

/// Walk one value, reading it out when the client has a use for it.
///
/// Returns `false` only when the shape itself could not be walked (a nested
/// unknown component or the depth limit) — a value that is walked but not
/// interpreted is a success.
fn read_interpreted(
    r: &mut PacketReader,
    ty: i32,
    ids: DataComponentIds,
    shape: &crate::component_wire::Shape,
    out: &mut StackComponents,
) -> Result<bool, ()> {
    use crate::component_wire::{
        nbt_text, read_container_slot_list, read_item_template_list, walk,
    };
    if ty == ids.damage {
        out.damage = Some(r.varint().map_err(|_| ())?);
        return Ok(true);
    }
    if ty == ids.max_damage {
        out.max_damage = Some(r.varint().map_err(|_| ())?);
        return Ok(true);
    }
    if ty == ids.rarity {
        out.rarity = Some(r.varint().map_err(|_| ())?);
        return Ok(true);
    }
    if ty == ids.dyed_color {
        // `ByteBufCodecs.INT` — a fixed big-endian i32 among the var-ints,
        // the same trap `container_set_slot`'s signed short is.
        out.dyed_color = Some(r.i32().map_err(|_| ())?);
        return Ok(true);
    }
    if ty == ids.enchantment_glint_override {
        out.glint_override = Some(r.u8().map_err(|_| ())? != 0);
        return Ok(true);
    }
    if ty == ids.unbreakable {
        // `Unit` — zero bytes. The presence of the entry is the value.
        out.unbreakable = true;
        return Ok(true);
    }
    if ty == ids.custom_name || ty == ids.item_name {
        let tag = rewo_proto::nbt::Nbt::read_network(r).map_err(|_| ())?;
        let text = nbt_text(&tag);
        if ty == ids.custom_name {
            out.custom_name = Some(text);
        } else {
            out.item_name = Some(text);
        }
        return Ok(true);
    }
    if ty == ids.lore {
        let n = r.varint().map_err(|_| ())?;
        if !(0..=256).contains(&n) {
            return Err(());
        }
        for _ in 0..n {
            let tag = rewo_proto::nbt::Nbt::read_network(r).map_err(|_| ())?;
            out.lore.push(nbt_text(&tag));
        }
        return Ok(true);
    }
    if ty == ids.trim {
        // Two `ByteBufCodecs.holder`s: `id + 1`, 0 = inline. Peeked rather
        // than consumed — if either side is inline the generic walk has to see
        // the whole component from the start, so this rewinds and defers.
        let save = r.offset();
        let m = r.varint().map_err(|_| ())?;
        let p = if m > 0 { r.varint().map_err(|_| ())? } else { 0 };
        if m > 0 && p > 0 {
            out.trim = Some((m - 1, p - 1));
            return Ok(true);
        }
        r.rewind_to(save);
        return walk(r, shape, 0);
    }
    if ty == ids.bundle_contents {
        // `BundleContents.STREAM_CODEC` is `ItemStackTemplate.STREAM_CODEC
        // .apply(ByteBufCodecs.list())` — a var-int count then that many
        // templates, and nothing more. `selectedItem` is not on the wire.
        //
        // `read_item_template_list` shares its body with the generic walk's
        // `Shape::List(&Shape::ItemStackTemplate)`, so capturing here consumes
        // byte-for-byte what walking past here consumed before. That identity
        // is the point: the patch has no length prefix, so a capture that
        // read one byte differently would park the reader mid-value and turn
        // every later slot in the packet into garbage.
        //
        // Depth 0 to match the generic fall-through's `walk(r, shape, 0)`
        // below — the outer patch's entries are the top of a fresh recursion
        // budget, and a bundle nested inside one of these elements is walked
        // (not captured) under that budget like any other component.
        return Ok(match read_item_template_list(r, 0)? {
            Some(items) => {
                out.bundle = Some(items);
                true
            }
            // A nested patch named a component with no codec, or the depth
            // limit stopped it. The reader is parked mid-value, so this is the
            // same fail-closed answer an unknown top-level component gets.
            None => false,
        });
    }
    if ty == ids.container {
        // `ItemContainerContents.STREAM_CODEC` is `ItemStackTemplate
        // .STREAM_CODEC.apply(ByteBufCodecs::optional).apply(list(256))` — a
        // var-int count, then per slot a presence bool and, if set, a template.
        //
        // `read_container_slot_list` shares its patch loop with the generic
        // walk through `walk_patch_with`, so capturing here consumes
        // byte-for-byte what `Shape::List(&Shape::Optional(&ItemStackTemplate))`
        // consumed before — the same identity `bundle_contents` relies on, and
        // for the same reason: the patch has no length prefix, so a capture
        // that read one byte differently would park the reader mid-value and
        // turn every later slot in the packet into garbage.
        //
        // Depth 0 to match the generic fall-through's `walk(r, shape, 0)`.
        return Ok(match read_container_slot_list(r, 0, ids.custom_name)? {
            Some(slots) => {
                out.container = Some(slots);
                true
            }
            // A nested patch named a component with no codec, or the depth
            // limit stopped it — the same fail-closed answer an unknown
            // top-level component gets.
            None => false,
        });
    }
    if ty == ids.enchantments || ty == ids.stored_enchantments {
        let n = r.varint().map_err(|_| ())?;
        if !(0..=65536).contains(&n) {
            return Err(());
        }
        // `isEnchanted` reads `ENCHANTMENTS` alone. The two components share
        // this branch because they share a codec, not a meaning.
        out.is_enchanted |= ty == ids.enchantments && n > 0;
        for _ in 0..n {
            // `Enchantment.STREAM_CODEC` is `holderRegistry` — a **raw** id,
            // not `holder`'s `id + 1`.
            let id = r.varint().map_err(|_| ())?;
            let level = r.varint().map_err(|_| ())?;
            out.enchantments.push((id, level));
        }
        return Ok(true);
    }
    walk(r, shape, 0)
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
        let (outcome, _, _, _) = read_patch_at(r, ids, depth + 1)?;
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
        max_damage: 4,
        rarity: 5,
        unbreakable: 6,
        custom_name: 8,
        item_name: 9,
        lore: 10,
        enchantments: 11,
        stored_enchantments: 12,
        enchantment_glint_override: 13,
        dyed_color: 14,
        trim: 15,
        bundle_contents: 16,
        container: 17,
    };

    /// The walk is table-driven now, and the table is keyed by *name* against
    /// the live registry — so these unit fixtures have to install shapes for
    /// the ids they use, or every component would read as unwalkable.
    fn install_test_shapes() {
        let ids: std::collections::HashMap<String, i32> = [
            ("minecraft:swing_animation", 40),
            ("minecraft:damage", 3),
            ("minecraft:charged_projectiles", 7),
            ("minecraft:max_damage", 4),
            ("minecraft:rarity", 5),
            ("minecraft:unbreakable", 6),
            ("minecraft:custom_name", 8),
            ("minecraft:item_name", 9),
            ("minecraft:lore", 10),
            ("minecraft:enchantments", 11),
            ("minecraft:enchantment_glint_override", 13),
            ("minecraft:dyed_color", 14),
            ("minecraft:bundle_contents", 16),
            ("minecraft:container", 17),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        crate::component_wire::install_shapes(&ids);
    }


    /// `DyedItemColor.STREAM_CODEC` is `ByteBufCodecs.INT` — a **fixed**
    /// big-endian i32 among the var-ints, the same trap `container_set_slot`'s
    /// signed short is. Read as a var-int, 0xB02E26 would consume three bytes
    /// and leave the fourth to be parsed as the next component's type id.
    #[test]
    fn dyed_color_is_a_fixed_i32_not_a_varint() {
        install_test_shapes();
        let raw = stack(1, 1, &[(14, vec![0x00, 0xB0, 0x2E, 0x26])], &[]);
        let mut r = PacketReader::new(&raw);
        let slot = read_optional(&mut r, IDS).expect("decodes");
        let WireSlot::Stack(s) = slot else {
            panic!("expected a stack");
        };
        assert_eq!(s.components.dyed_color, Some(0x00B0_2E26));
        // The whole patch was consumed: nothing is left for a later entry to
        // misread.
        assert_eq!(r.remaining(), 0);
    }

    /// Absence is **not** a black dye. `getOrDefault` answers 0 for an absent
    /// component, and 0 is what `getColorForLayer` reads as "undyed" — which
    /// sends a dyeable layer to its `color_when_undyed` rather than tinting it
    /// black.
    #[test]
    fn an_undyed_stack_carries_no_dye_rather_than_zero() {
        install_test_shapes();
        let raw = stack(1, 1, &[], &[]);
        let mut r = PacketReader::new(&raw);
        let WireSlot::Stack(s) = read_optional(&mut r, IDS).expect("decodes") else {
            panic!("expected a stack");
        };
        assert_eq!(s.components.dyed_color, None);
        assert_eq!(rewo_data::equipment::dye_argb(s.components.dyed_color), 0);
        // ...and a *black* dye is a real dye, distinct from absence.
        assert_eq!(rewo_data::equipment::dye_argb(Some(0)), 0xFF00_0000);
    }

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

    fn patch_of(slot: &WireSlot) -> PatchOutcome {
        match slot {
            WireSlot::Stack(s) => s.patch,
            WireSlot::Empty => panic!("expected a stack"),
        }
    }

    #[test]
    fn non_positive_count_is_the_empty_stack_and_reads_nothing_more() {
        install_test_shapes();
        for count in [0, -1, -128] {
            let mut b = Vec::new();
            varint(count, &mut b);
            assert_eq!(read(&b), Ok(WireSlot::Empty), "count {count}");
        }
    }

    #[test]
    fn a_plain_stack_has_an_empty_patch() {
        install_test_shapes();
        let s = read(&stack(1, 949, &[], &[])).unwrap();
        assert!(s.aligned());
        assert_eq!(patch_of(&s), PatchOutcome::Walked(PatchSwing::Absent));
    }

    #[test]
    fn an_explicit_swing_animation_override_is_decoded() {
        install_test_shapes();
        let s = read(&stack(1, 100, &[(IDS.swing_animation, swing_value(2, 11))], &[])).unwrap();
        assert_eq!(
            patch_of(&s),
            PatchOutcome::Walked(PatchSwing::Set(SwingAnimation::new(
                SwingAnimationType::Stab,
                11
            )))
        );
    }

    #[test]
    fn the_walk_continues_past_the_swing_component_and_stays_aligned() {
        install_test_shapes();
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
            patch_of(&slot),
            PatchOutcome::Walked(PatchSwing::Set(SwingAnimation::new(
                SwingAnimationType::Stab,
                11
            )))
        );
        assert!(r.u8().is_err(), "the whole patch was consumed");
    }

    #[test]
    fn damage_is_walked_past_to_reach_a_later_override() {
        install_test_shapes();
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
            patch_of(&s),
            PatchOutcome::Walked(PatchSwing::Set(SwingAnimation::new(
                SwingAnimationType::None,
                4
            )))
        );
    }

    /// A component id no registry can contain, so the walk has no shape for it.
    ///
    /// The three tests below used a hard-coded `13` until M43 gave that id a
    /// codec, at which point they silently started asserting the opposite of
    /// their names. An impossible id cannot rot the same way: the property is
    /// "an id with no shape stops the walk", not "this component happens to be
    /// uncovered today".
    const NO_SUCH_COMPONENT: i32 = i32::MAX;

    #[test]
    fn an_unknown_component_before_the_swing_stops_the_walk() {
        install_test_shapes();
        let s = read(&stack(
            1,
            100,
            &[(NO_SUCH_COMPONENT, vec![0xAA, 0xBB, 0xCC])],
            &[IDS.swing_animation],
        ))
        .unwrap();
        assert_eq!(patch_of(&s), PatchOutcome::Unwalkable);
        assert!(!s.aligned());
    }

    #[test]
    fn an_unknown_component_after_the_swing_still_stops_the_walk() {
        install_test_shapes();
        // The override *was* read, but the reader is now stuck: reporting
        // `Walked` here would desynchronise the packet, so the whole stack is
        // Unwalkable and its swing unknown.
        let body = stack(
            1,
            100,
            &[
                (IDS.swing_animation, swing_value(2, 11)),
                (NO_SUCH_COMPONENT, vec![0xAA, 0xBB]),
            ],
            &[],
        );
        let s = read(&body).unwrap();
        assert_eq!(patch_of(&s), PatchOutcome::Unwalkable);
        assert!(!s.aligned());
    }

    #[test]
    fn an_unresolved_patch_leaves_the_reader_mid_value() {
        install_test_shapes();
        // The three junk bytes after the un-transcribed component are NOT
        // consumed — which is exactly why the caller must stop rather than
        // read a second slot out of them.
        let bytes = stack(1, 100, &[(NO_SUCH_COMPONENT, vec![0xAA, 0xBB, 0xCC])], &[]);
        let mut r = PacketReader::new(&bytes);
        let s = read_optional(&mut r, IDS).unwrap();
        assert!(!s.aligned());
        assert_eq!(r.u8().ok(), Some(0xAA), "the component's value is still unread");
    }

    #[test]
    fn a_removal_only_patch_is_fully_walkable() {
        install_test_shapes();
        let s = read(&stack(1, 100, &[], &[3, IDS.swing_animation, 19])).unwrap();
        assert_eq!(patch_of(&s), PatchOutcome::Walked(PatchSwing::Removed));
        // …and a removal list without the swing component is Absent.
        let s = read(&stack(1, 100, &[], &[3, 19])).unwrap();
        assert_eq!(patch_of(&s), PatchOutcome::Walked(PatchSwing::Absent));
    }

    #[test]
    fn a_truncated_stack_is_an_error_not_a_guess() {
        install_test_shapes();
        assert_eq!(read(&[]), Err(()));
        assert_eq!(read(&[1]), Err(())); // count but no item
        assert_eq!(read(&[1, 100]), Err(())); // item but no patch header
        assert_eq!(read(&[1, 100, 1, 0]), Err(())); // added=1 but no type
    }

    // `resolve_swing`'s prototype/unregistered arms need a *real* item registry
    // (a spear whose prototype differs from the default, and an id outside the
    // registry) to be worth asserting, so they are witnessed in
    // `rewo swingshot --check` rather than against a stand-in table here.

    // ---- bundle_contents (M61) --------------------------------------------

    /// A `minecraft:bundle_contents` value: a var-int count then that many
    /// `ItemStackTemplate`s, each `(raw item id, count, patch)`.
    fn bundle_value(items: &[(i32, i32)]) -> Vec<u8> {
        let mut v = Vec::new();
        varint(items.len() as i32, &mut v);
        for (item, count) in items {
            varint(*item, &mut v);
            varint(*count, &mut v);
            varint(0, &mut v); // added
            varint(0, &mut v); // removed
        }
        v
    }

    fn components_of(raw: &[u8]) -> StackComponents {
        let mut r = PacketReader::new(raw);
        let WireSlot::Stack(s) = read_optional(&mut r, IDS).expect("decodes") else {
            panic!("expected a stack");
        };
        assert_eq!(r.remaining(), 0, "the patch was not consumed exactly");
        s.components
    }

    /// The whole point of M61: the stacks survive the walk instead of being
    /// counted and thrown away.
    #[test]
    fn a_bundle_patch_keeps_the_stacks_it_holds() {
        install_test_shapes();
        let raw = stack(1, 800, &[(16, bundle_value(&[(1, 64), (2, 1), (3, 12)]))], &[]);
        let c = components_of(&raw);
        let items = c.bundle_contents().expect("the bundle was captured");
        assert_eq!(items.len(), 3);
        assert_eq!(
            items.iter().map(|i| (i.item_id, i.count)).collect::<Vec<_>>(),
            vec![(1, 64), (2, 1), (3, 12)]
        );
    }

    /// **The alignment witness.** A bundle sized wrong leaves the reader
    /// parked mid-value, and the patch has no length prefix to recover from —
    /// so the entry *after* it is the thing that notices.
    ///
    /// Reading the damage back is a stronger claim than "the patch walked":
    /// an off-by-one that happened to land on a valid var-int would still
    /// report `Walked`, and would report the wrong durability.
    #[test]
    fn a_bundle_entry_leaves_the_reader_aligned_for_the_component_after_it() {
        install_test_shapes();
        let raw = stack(
            1,
            800,
            &[
                (16, bundle_value(&[(1, 64), (2, 1)])),
                // 300 — two var-int bytes, so a one-byte slip is visible in
                // the value rather than only in the alignment.
                (3, vec![0xAC, 0x02]),
            ],
            &[],
        );
        let c = components_of(&raw);
        assert_eq!(c.bundle_contents().map(<[_]>::len), Some(2));
        assert_eq!(c.damage, Some(300));
    }

    /// …and the same in the other order, because a bundle read short and a
    /// bundle read long fail differently: this one puts the slip *before* the
    /// component the assertion reads.
    #[test]
    fn a_component_before_a_bundle_does_not_disturb_it() {
        install_test_shapes();
        let raw = stack(
            1,
            800,
            &[
                (3, vec![0xAC, 0x02]),
                (16, bundle_value(&[(1, 64), (2, 1)])),
            ],
            &[],
        );
        let c = components_of(&raw);
        assert_eq!(c.damage, Some(300));
        assert_eq!(c.bundle_contents().map(<[_]>::len), Some(2));
    }

    /// Three states, not two. Absence resolves through `BundleContents.EMPTY`
    /// and so does a removal; an explicitly empty list is the server saying
    /// the bundle *is* empty, which vanilla draws as the empty-bundle blurb
    /// rather than as no tooltip image.
    #[test]
    fn an_absent_bundle_is_not_the_same_as_an_empty_one() {
        install_test_shapes();
        assert_eq!(components_of(&stack(1, 800, &[], &[])).bundle, None);
        assert_eq!(
            components_of(&stack(1, 800, &[(16, bundle_value(&[]))], &[])).bundle,
            Some(Vec::new())
        );
        // A removal leaves `bundle` at `None` and records the id, so a caller
        // can tell "removed" from "never mentioned" if it ever needs to.
        let removed = components_of(&stack(1, 800, &[], &[16]));
        assert_eq!(removed.bundle, None);
        assert_eq!(removed.removed, vec![16]);
    }

    /// Two bundles differing only in their contents are different components,
    /// and `isSameItemSameComponents` has to say so or they would merge into
    /// one slot in a click prediction.
    ///
    /// This holds because the fingerprint spans the value's *bytes*, which is
    /// only true while the capture consumes exactly the value — so it is a
    /// second, independent reading of the alignment property above.
    #[test]
    fn two_bundles_holding_different_stacks_fingerprint_differently() {
        install_test_shapes();
        let a = components_of(&stack(1, 800, &[(16, bundle_value(&[(1, 64)]))], &[]));
        let b = components_of(&stack(1, 800, &[(16, bundle_value(&[(1, 63)]))], &[]));
        let same = components_of(&stack(1, 800, &[(16, bundle_value(&[(1, 64)]))], &[]));
        assert_ne!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint, same.fingerprint);
    }

    /// An element whose patch names a component with no codec stops the whole
    /// stack, exactly as one at the top level does. The bundle is *not*
    /// reported as the stacks read so far: a partial bundle presented as a
    /// whole one would be a confident wrong answer, which is the failure mode
    /// this decoder is built to refuse.
    #[test]
    fn an_unwalkable_component_inside_a_bundle_makes_the_stack_unwalkable() {
        install_test_shapes();
        let mut value = Vec::new();
        varint(2, &mut value);
        varint(1, &mut value); // first element: item
        varint(64, &mut value); // count
        varint(0, &mut value);
        varint(0, &mut value);
        varint(2, &mut value); // second element: item
        varint(1, &mut value); // count
        varint(1, &mut value); // added
        varint(0, &mut value); // removed
        varint(999, &mut value); // a component no test registry installs
        value.extend_from_slice(&[0xAA, 0xBB]);
        let raw = stack(1, 800, &[(16, value)], &[]);
        let mut r = PacketReader::new(&raw);
        let WireSlot::Stack(s) = read_optional(&mut r, IDS).expect("decodes") else {
            panic!("expected a stack");
        };
        assert_eq!(s.patch, PatchOutcome::Unwalkable);
        assert!(!s.aligned_stack());
        assert_eq!(s.components.bundle, None);
    }

    // ---- container (M63) --------------------------------------------------

    /// A `{"text": s}` chat component in network-NBT form.
    fn text_tag(s: &str) -> Vec<u8> {
        let mut v = vec![0x0A, 0x08];
        v.extend_from_slice(&4u16.to_be_bytes());
        v.extend_from_slice(b"text");
        v.extend_from_slice(&(s.len() as u16).to_be_bytes());
        v.extend_from_slice(s.as_bytes());
        v.push(0x00);
        v
    }

    /// A `minecraft:container` value: a var-int count, then per slot a
    /// presence bool and — when present — an `ItemStackTemplate` whose patch
    /// optionally carries a `custom_name`.
    fn container_value(slots: &[Option<(i32, i32, Option<&str>)>]) -> Vec<u8> {
        let mut v = Vec::new();
        varint(slots.len() as i32, &mut v);
        for s in slots {
            match s {
                None => v.push(0),
                Some((item, count, name)) => {
                    v.push(1);
                    varint(*item, &mut v);
                    varint(*count, &mut v);
                    varint(name.is_some() as i32, &mut v); // added
                    varint(0, &mut v); // removed
                    if let Some(n) = name {
                        varint(IDS.custom_name, &mut v);
                        v.extend_from_slice(&text_tag(n));
                    }
                }
            }
        }
        v
    }

    /// The whole point of M63: a shulker box's slots survive the walk instead
    /// of being counted and thrown away, hover names included.
    #[test]
    fn a_container_patch_keeps_the_slots_it_holds() {
        install_test_shapes();
        let raw = stack(
            1,
            600,
            &[(
                17,
                container_value(&[
                    Some((1, 64, None)),
                    None,
                    Some((2, 1, Some("Excalibur"))),
                ]),
            )],
            &[],
        );
        let c = components_of(&raw);
        let slots = c.container_contents().expect("the container was captured");
        assert_eq!(slots.len(), 3, "the empty slot was dropped");
        assert!(slots[1].is_none());
        assert_eq!(slots[0].as_ref().unwrap().item_id, 1);
        assert_eq!(slots[0].as_ref().unwrap().count, 64);
        assert_eq!(
            slots[2].as_ref().unwrap().custom_name.as_deref(),
            Some("Excalibur")
        );
        // `addToTooltip` walks only the occupied ones.
        assert_eq!(c.container_items().count(), 2);
    }

    /// **The alignment witness**, and the property that matters most: the
    /// patch has no length prefix, so a container sized wrong parks the
    /// reader and the entry *after* it is what notices.
    ///
    /// Reading the damage back is the stronger claim — an off-by-one landing
    /// on a valid var-int would still report `Walked`, with the wrong value.
    /// The named slot is deliberately in the middle, because the nested tag is
    /// the one span the capture reads itself rather than through `Shape`.
    #[test]
    fn a_container_entry_leaves_the_reader_aligned_for_the_component_after_it() {
        install_test_shapes();
        let raw = stack(
            1,
            600,
            &[
                (
                    17,
                    container_value(&[
                        Some((1, 64, None)),
                        Some((2, 1, Some("Bag of Holding"))),
                        None,
                        Some((3, 12, None)),
                    ]),
                ),
                // 300 — two var-int bytes, so a one-byte slip shows up in the
                // value and not only in the alignment.
                (3, vec![0xAC, 0x02]),
            ],
            &[],
        );
        let c = components_of(&raw);
        assert_eq!(c.container_contents().map(<[_]>::len), Some(4));
        assert_eq!(c.container_items().count(), 3);
        assert_eq!(c.damage, Some(300));
    }

    /// …and the same with the slip placed *before* the assertion's component,
    /// because a container read short and one read long fail differently.
    #[test]
    fn a_component_before_a_container_does_not_disturb_it() {
        install_test_shapes();
        let raw = stack(
            1,
            600,
            &[
                (3, vec![0xAC, 0x02]),
                (17, container_value(&[Some((1, 64, Some("Tools"))), None])),
            ],
            &[],
        );
        let c = components_of(&raw);
        assert_eq!(c.damage, Some(300));
        assert_eq!(c.container_contents().map(<[_]>::len), Some(2));
        assert_eq!(
            c.container_items().next().unwrap().custom_name.as_deref(),
            Some("Tools")
        );
    }

    /// Three states, exactly as for a bundle: absence and a removal both
    /// resolve through `ItemContainerContents.EMPTY`, while an explicitly
    /// empty list is the server saying the container *is* empty.
    #[test]
    fn an_absent_container_is_not_the_same_as_an_empty_one() {
        install_test_shapes();
        assert_eq!(components_of(&stack(1, 600, &[], &[])).container, None);
        assert_eq!(
            components_of(&stack(1, 600, &[(17, container_value(&[]))], &[])).container,
            Some(Vec::new())
        );
        let removed = components_of(&stack(1, 600, &[], &[17]));
        assert_eq!(removed.container, None);
        assert_eq!(removed.removed, vec![17]);
    }

    /// Two containers differing only in a slot's name are different
    /// components. This holds because the fingerprint spans the value's
    /// *bytes*, which is only true while the capture consumes exactly the
    /// value — so it reads the alignment property a second, independent way.
    #[test]
    fn two_containers_differing_only_in_a_slot_name_fingerprint_differently() {
        install_test_shapes();
        let named = |n: &str| {
            components_of(&stack(
                1,
                600,
                &[(17, container_value(&[Some((1, 1, Some(n)))]))],
                &[],
            ))
        };
        assert_ne!(named("Alpha").fingerprint, named("Beta").fingerprint);
        assert_eq!(named("Alpha").fingerprint, named("Alpha").fingerprint);
    }

    /// A slot whose patch names a component with no codec stops the whole
    /// stack. The container is *not* reported as the slots read so far — a
    /// partial container presented as a whole one is a confident wrong answer.
    #[test]
    fn an_unwalkable_component_inside_a_container_slot_makes_the_stack_unwalkable() {
        install_test_shapes();
        let mut value = Vec::new();
        varint(1, &mut value); // one slot
        value.push(1); // present
        varint(2, &mut value); // item
        varint(1, &mut value); // count
        varint(1, &mut value); // added
        varint(0, &mut value); // removed
        varint(999, &mut value); // a component no test registry installs
        value.extend_from_slice(&[0xAA, 0xBB]);
        let raw = stack(1, 600, &[(17, value)], &[]);
        let mut r = PacketReader::new(&raw);
        let WireSlot::Stack(s) = read_optional(&mut r, IDS).expect("decodes") else {
            panic!("expected a stack");
        };
        assert_eq!(s.patch, PatchOutcome::Unwalkable);
        assert!(!s.aligned_stack());
        assert_eq!(s.components.container, None);
    }
}
