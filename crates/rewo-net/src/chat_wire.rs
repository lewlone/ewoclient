//! `player_chat`, `delete_chat` and the signature cache between them (M108).
//!
//! Before this, `player_chat` was a **prefix parse**: global index, sender,
//! index, an optionally-skipped 256-byte signature, and the body's content
//! string — then it stopped, leaving two thirds of the packet unread and
//! throwing the signature away. That was enough to put a line on screen and
//! not enough for anything else, which is why `delete_chat` sat in
//! `REWO_PACKET_COVERAGE.md` as class C: with no signature stored there is
//! nothing to delete.
//!
//! # `delete_chat` cannot be read without the cache
//!
//! `MessageSignature.Packed.read` is
//!
//! ```java
//! int id = input.readVarInt() - 1;
//! return id == -1 ? new Packed(MessageSignature.read(input)) : new Packed(id);
//! ```
//!
//! so wire `0` means **a full 256-byte signature follows inline** and anything
//! else is an **index into the client's own `MessageSignatureCache`**. This is
//! the `id + 1` shape M52e's `holder` and M93y's `OPTIONAL_VAR_INT` already
//! record, with the inline branch real rather than an absence — and it makes
//! the packet unreadable in isolation: a server that has seen you acknowledge a
//! signature will refer to it by index, and a client with no cache has no way
//! back to the 256 bytes. So the cache is not an optimisation to skip; it is a
//! prerequisite.
//!
//! **`MessageSignatureCache.unpack` is `this.entries[id]` with no bounds
//! check** — vanilla throws `ArrayIndexOutOfBoundsException` on a hostile id.
//! [`MessageSignatureCache::unpack`] returns `None` instead. That is a
//! deliberate divergence: crashing on a malformed packet is not behaviour worth
//! reproducing, and every caller here already has a "no such signature" path
//! because a *valid* id can point at an empty slot.
//!
//! # The cache is a move-to-front LRU that dedupes, not a ring
//!
//! ```java
//! Set<MessageSignature> newEntries = new ObjectOpenHashSet(queue);
//! for (int i = 0; !queue.isEmpty() && i < this.entries.length; i++) {
//!    MessageSignature entry = this.entries[i];
//!    this.entries[i] = queue.removeLast();
//!    if (entry != null && !newEntries.contains(entry)) {
//!       queue.addFirst(entry);
//!    }
//! }
//! ```
//!
//! Each slot from 0 up is overwritten with `queue.removeLast()` — the *newest*
//! arrival first, because `push` builds the queue as `lastSeen ++ [signature]` —
//! and the displaced entry is pushed back onto the **front** of the queue so it
//! slides down one slot, *unless* it is among the arrivals, in which case it is
//! dropped and everything below it moves up. A plain ring buffer agrees only
//! while nothing repeats.
//!
//! # `overlay` is not a chat message
//!
//! `handleSystemChat` branches on it: `true` goes to `handleOverlay`, which is
//! `gui.setOverlayMessage` — the **action bar**. Rewo read the component and
//! discarded the bool, so every `/title @s actionbar` and every plugin status
//! line was appearing in the chat log as well as (or instead of) above the
//! hotbar.
//!
//! # What is decoded and deliberately not acted on
//!
//! The **decoration** — `boundChatType.decorate(content)`, which is what turns
//! a bare message into `<Steve> hi`. M78 recorded the blocker and it is
//! unchanged: it needs the `minecraft:chat_type` registry's *contents* from
//! configuration `registry_data`, which Rewo does not parse for that registry,
//! plus the language table, which `rewo-net` cannot see. Both halves are now
//! *reachable* (M42 parses one datapack registry, M54 loads the language map),
//! so this is a scheduling decision rather than a wall — named here so it is
//! not mistaken for an oversight. Until then a player line renders as its
//! content, which is what the pre-M108 code did too.

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `MessageSignature.BYTES`.
pub const SIGNATURE_BYTES: usize = 256;

/// `MessageSignature` — compared by value, never interpreted here.
pub type Signature = [u8; SIGNATURE_BYTES];

/// `MessageSignatureCache.DEFAULT_CAPACITY`.
pub const CACHE_CAPACITY: usize = 128;

/// `LastSeenMessages.LAST_SEEN_MESSAGES_MAX_LENGTH` — the cap
/// `readCollection(limitValue(ArrayList::new, 20), …)` enforces.
pub const LAST_SEEN_MAX: usize = 20;

/// `PlayerChatMessage.MESSAGE_EXPIRES_AFTER_CLIENT` — five minutes plus two,
/// in milliseconds. A message older than this is `NOT_SECURE` however well it
/// is signed.
pub const MESSAGE_EXPIRES_AFTER_CLIENT_MILLIS: i64 = (5 + 2) * 60 * 1000;

/// `MessageSignature.Packed` — either the bytes, or an index into the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackedSignature {
    /// Wire `0`: the 256 bytes follow inline.
    Full(Box<Signature>),
    /// Wire `n > 0`: cache slot `n - 1`.
    Cached(i32),
}

/// `MessageSignature.read` — 256 raw bytes, no length prefix.
pub fn read_signature(r: &mut PacketReader<'_>) -> Result<Box<Signature>> {
    let bytes = r.take(SIGNATURE_BYTES)?;
    let mut out = Box::new([0u8; SIGNATURE_BYTES]);
    out.copy_from_slice(bytes);
    Ok(out)
}

/// `MessageSignature.Packed.read`.
pub fn read_packed_signature(r: &mut PacketReader<'_>) -> Result<PackedSignature> {
    let id = r.varint()? - 1;
    if id == -1 {
        Ok(PackedSignature::Full(read_signature(r)?))
    } else {
        Ok(PackedSignature::Cached(id))
    }
}

/// `MessageSignatureCache`.
#[derive(Clone)]
pub struct MessageSignatureCache {
    entries: Vec<Option<Box<Signature>>>,
}

impl Default for MessageSignatureCache {
    fn default() -> Self {
        Self::new(CACHE_CAPACITY)
    }
}

impl std::fmt::Debug for MessageSignatureCache {
    /// 128 × 256 bytes is not something to print. The occupancy is.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let filled = self.entries.iter().filter(|e| e.is_some()).count();
        write!(
            f,
            "MessageSignatureCache {{ {filled}/{} }}",
            self.entries.len()
        )
    }
}

impl MessageSignatureCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: vec![None; capacity],
        }
    }

    /// `pack` — the slot holding this signature, or `None` for
    /// `NOT_FOUND` (-1). Serverbound only; kept because it is the inverse the
    /// tests grade [`Self::push`] with.
    pub fn pack(&self, signature: &Signature) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.as_deref().is_some_and(|s| s == signature))
    }

    /// `unpack` — **bounds-checked**, unlike vanilla, which indexes the array
    /// raw and throws on a hostile id. See the module docs.
    pub fn unpack(&self, id: i32) -> Option<&Signature> {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.entries.get(i))
            .and_then(|e| e.as_deref())
    }

    /// Resolve a [`PackedSignature`] against this cache.
    pub fn resolve(&self, packed: &PackedSignature) -> Option<Box<Signature>> {
        match packed {
            PackedSignature::Full(s) => Some(s.clone()),
            PackedSignature::Cached(id) => self.unpack(*id).map(|s| Box::new(*s)),
        }
    }

    /// `push(SignedMessageBody, signature)` — the queue is
    /// `lastSeen ++ [signature]`, so the message's own signature is what
    /// `removeLast` hands out first and therefore what lands in slot 0.
    pub fn push(&mut self, last_seen: &[Box<Signature>], signature: Option<&Signature>) {
        let mut queue: std::collections::VecDeque<Box<Signature>> =
            last_seen.iter().cloned().collect();
        if let Some(s) = signature {
            queue.push_back(Box::new(*s));
        }
        self.push_queue(queue);
    }

    fn push_queue(&mut self, mut queue: std::collections::VecDeque<Box<Signature>>) {
        // `new ObjectOpenHashSet(queue)` — snapshotted before the walk, so an
        // entry displaced into the queue does not later count as "new".
        let new_entries: Vec<Box<Signature>> = queue.iter().cloned().collect();
        let mut i = 0usize;
        while !queue.is_empty() && i < self.entries.len() {
            let displaced = self.entries[i].take();
            self.entries[i] = queue.pop_back();
            if let Some(entry) = displaced {
                if !new_entries.iter().any(|n| **n == *entry) {
                    queue.push_front(entry);
                }
            }
            i += 1;
        }
    }
}

/// `FilterMask`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterMask {
    PassThrough,
    FullyFiltered,
    /// The `BitSet`'s backing longs, exactly as `readBitSet` produced them.
    PartiallyFiltered(Vec<i64>),
}

impl FilterMask {
    /// `FilterMask.read`.
    ///
    /// The discriminant is `readEnum`, which is
    /// `getEnumConstants()[readVarInt()]` — so an out-of-range value **throws**
    /// in vanilla. M65 recorded the pair of conventions this sits between:
    /// `readEnum` errors where `ByIdMap.continuous(…, ZERO)` silently defaults.
    /// Reading this one as forgiving would accept a body vanilla rejects and
    /// then read the following field from the wrong offset.
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        match r.varint()? {
            0 => Ok(Self::PassThrough),
            1 => Ok(Self::FullyFiltered),
            2 => Ok(Self::PartiallyFiltered(r.long_array()?)),
            other => Err(rewo_proto::ProtoError::Frame(format!(
                "FilterMask.Type ordinal {other} out of range"
            ))),
        }
    }

    /// `isEmpty()` — **`PASS_THROUGH`, not "masks nothing"**. A
    /// `PARTIALLY_FILTERED` whose bits are all clear is *not* empty, and the
    /// difference is observable: `showMessageToPlayer` takes the
    /// `filterMask.isEmpty()` branch to render the **decorated** (possibly
    /// unsigned-overridden) content, and the other branch to render the
    /// **signed** content. So a server sending an all-clear partial mask
    /// suppresses its own unsigned override.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::PassThrough)
    }

    pub fn is_fully_filtered(&self) -> bool {
        matches!(self, Self::FullyFiltered)
    }

    fn bit(longs: &[i64], index: usize) -> bool {
        longs
            .get(index / 64)
            .is_some_and(|w| (*w as u64) >> (index % 64) & 1 == 1)
    }

    /// `FilterMask.applyWithFormatting` — `None` is vanilla's `null`, i.e.
    /// **show nothing**.
    ///
    /// **A component, not a string, and that is the method the client calls.**
    /// `showMessageToPlayer`'s masked branch is
    /// `filterMask.applyWithFormatting(message.signedContent())`, which builds
    /// `Component.empty()` and appends alternating runs: an unfiltered run as a
    /// bare `append(String)` (a literal with `Style.EMPTY`, so it inherits),
    /// and a filtered run as a `#`-repeat carrying **`FILTERED_STYLE`** —
    /// `Style.EMPTY.withColor(DARK_GRAY).withHoverEvent(ShowText("chat.filtered"))`.
    ///
    /// Rewo used `FilterMask.apply`, the *string* sibling, which exists for the
    /// server's log line. The visible difference is the colour: vanilla's `#`s
    /// are dark grey against the message's own colour, and a string of `#`s
    /// takes whatever colour surrounds it. The hover half is not modelled —
    /// [`rewo_world::chat_style::ChatStyle`] carries no events — so a filtered
    /// run has the colour and not the tooltip.
    ///
    /// # The index basis is UTF-16, and Rewo's used to be scalar values
    ///
    /// Vanilla indexes with `BitSet.nextSetBit` / `nextClearBit` against
    /// `String.substring`, i.e. **Java char indices**, which are UTF-16 code
    /// units. Rewo's replaced `apply` walked `str::chars()`, so every bit past
    /// the first astral character (an emoji, say) addressed the wrong
    /// character. This walks code units.
    ///
    /// One stated approximation: a surrogate pair whose two units disagree
    /// cannot be reproduced, because vanilla's `substring` may split the pair
    /// and Rust's `String` cannot hold an unpaired surrogate. Such a scalar is
    /// filtered when **either** unit is set. Exact for every mask that does not
    /// cut a pair in half, which is every mask a word-boundary filter produces.
    pub fn apply_with_formatting(&self, text: &str) -> Option<rewo_proto::nbt::Nbt> {
        use rewo_proto::nbt::Nbt;
        match self {
            // `Component.literal(text)`.
            Self::PassThrough => Some(Nbt::String(text.to_string())),
            Self::FullyFiltered => None,
            Self::PartiallyFiltered(longs) => {
                let mut runs: Vec<Nbt> = Vec::new();
                let mut current = String::new();
                let mut current_filtered: Option<bool> = None;
                let mut unit = 0usize;
                for c in text.chars() {
                    let width = c.len_utf16();
                    // "either unit of a surrogate pair", per the doc above.
                    let filtered = (0..width).any(|k| Self::bit(longs, unit + k));
                    unit += width;
                    if current_filtered != Some(filtered) && !current.is_empty() {
                        runs.push(run(std::mem::take(&mut current), current_filtered == Some(true)));
                    }
                    current_filtered = Some(filtered);
                    // `StringUtils.repeat('#', nextIndex - previousIndex)` — the
                    // count is in the same units as the index, so an astral
                    // character is masked by **two** hashes, not one.
                    if filtered {
                        for _ in 0..width {
                            current.push('#');
                        }
                    } else {
                        current.push(c);
                    }
                }
                if !current.is_empty() {
                    runs.push(run(current, current_filtered == Some(true)));
                }
                // `Component.empty()` with the runs appended — a literal of ""
                // carrying children, which is what `MutableComponent.append`
                // produces and what an empty mask yields on its own.
                Some(Nbt::Compound(vec![
                    ("text".to_string(), Nbt::String(String::new())),
                    ("extra".to_string(), Nbt::List(runs)),
                ]))
            }
        }
    }
}


/// One run of `applyWithFormatting`'s alternation.
///
/// A filtered run carries `FILTERED_STYLE`'s colour; an unfiltered one is
/// `append(String)`, i.e. a literal with `Style.EMPTY`, which inherits.
fn run(text: String, filtered: bool) -> rewo_proto::nbt::Nbt {
    use rewo_proto::nbt::Nbt;
    if filtered {
        Nbt::Compound(vec![
            ("text".to_string(), Nbt::String(text)),
            ("color".to_string(), Nbt::String("dark_gray".to_string())),
        ])
    } else {
        Nbt::String(text)
    }
}

/// `SignedMessageBody.Packed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedMessageBody {
    pub content: String,
    /// `readInstant` — epoch **milliseconds** as a fixed i64.
    pub timestamp_millis: i64,
    pub salt: i64,
    pub last_seen: Vec<PackedSignature>,
}

impl SignedMessageBody {
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let content = r.string(256)?;
        let timestamp_millis = r.i64()?;
        let salt = r.i64()?;
        let count = r.varint()?;
        if !(0..=LAST_SEEN_MAX as i32).contains(&count) {
            return Err(rewo_proto::ProtoError::LengthOutOfRange {
                what: "lastSeen",
                len: count as i64,
                max: LAST_SEEN_MAX,
            });
        }
        let mut last_seen = Vec::with_capacity(count as usize);
        for _ in 0..count {
            last_seen.push(read_packed_signature(r)?);
        }
        Ok(Self {
            content,
            timestamp_millis,
            salt,
            last_seen,
        })
    }
}

/// One decoded `player_chat` body.
///
/// Not `Eq`: the bound's name is a component now (M127), and `Nbt` carries
/// floats.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerChat {
    pub global_index: i32,
    pub sender: u128,
    pub index: i32,
    pub signature: Option<Box<Signature>>,
    pub body: SignedMessageBody,
    /// The server's replacement rendering, **unflattened** (M127). `None` when
    /// absent, which is the common case.
    ///
    /// A component because `decoratedContent()` feeds the decoration, and a
    /// server that rewrites a message is exactly the server that colours the
    /// rewrite. Flattening it here also made `ChatTrustLevel`'s
    /// `containsModifiedStyle` half unreachable — see [`Self::trust_level`].
    pub unsigned_content: Option<rewo_proto::nbt::Nbt>,
    pub filter_mask: FilterMask,
    pub bound: crate::session::ChatTypeBound,
}

impl PlayerChat {
    /// `ClientboundPlayerChatPacket`'s constructor, in wire order.
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let global_index = r.varint()?;
        let sender = r.uuid()?;
        let index = r.varint()?;
        let signature = r.option(read_signature)?;
        let body = SignedMessageBody::read(r)?;
        let unsigned_content = r.option(|r| r.nbt())?;
        let filter_mask = FilterMask::read(r)?;
        let bound = crate::session::read_chat_type_bound(r)?;
        Ok(Self {
            global_index,
            sender,
            index,
            signature,
            body,
            unsigned_content,
            filter_mask,
            bound,
        })
    }

    /// `PlayerChatMessage.decoratedContent()` —
    /// `Objects.requireNonNullElseGet(unsignedContent, () -> Component.literal(signedContent))`.
    ///
    /// A **component**: the unsigned override when the server sent one, else a
    /// literal of the signed string. The literal branch is why a bare
    /// `Nbt::String` is the right fallback rather than a compound — that is
    /// exactly what `Component.literal` serialises to.
    pub fn decorated_content(&self) -> rewo_proto::nbt::Nbt {
        self.unsigned_content
            .clone()
            .unwrap_or_else(|| rewo_proto::nbt::Nbt::String(self.body.content.clone()))
    }

    /// `ChatTrustLevel.evaluate`, minus the one branch Rewo cannot see.
    ///
    /// **`decorated_plain` is the DECORATED message's text, not the content's**
    /// — `handlePlayerChatMessage` builds `decoratedMessage` and hands *that*
    /// to `evaluateTrustLevel`, whose `isModified` asks
    /// `!decoratedMessage.getString().contains(message.signedContent())`. So
    /// the question is whether `<Steve> hello` still contains `hello`, not
    /// whether `hello` does.
    ///
    /// Before M127 Rewo had no decoration and asked it of the undecorated
    /// content. The two agree for every ordinary message, because a decoration
    /// only wraps text around the content — and they diverge exactly when the
    /// decoration DROPS it: a translation key the language table does not
    /// carry, or a server-defined chat type whose parameter list omits
    /// `content`. Vanilla calls both of those MODIFIED. This is M92's shape —
    /// a value that could not be derived is derivable now.
    ///
    /// `isModified`'s second test is `containsModifiedStyle(unsignedContent)` —
    /// whether any style run names a non-default **font**. M127 stopped
    /// flattening `unsigned_content`, so the component survives; the half is
    /// still unreachable, but for a different and smaller reason:
    /// [`rewo_world::chat_style::ChatStyle`] models six visual fields and
    /// `font` is not one of them.
    ///
    /// `received_millis` is the client's clock, so a message stamped in the
    /// future is *not* expired — the test is one-sided (`now.isAfter(stamp +
    /// window)`).
    pub fn trust_level(&self, received_millis: i64, decorated_plain: &str) -> ChatTrustLevel {
        if self.signature.is_none()
            || received_millis
                > self.body.timestamp_millis + MESSAGE_EXPIRES_AFTER_CLIENT_MILLIS
        {
            return ChatTrustLevel::NotSecure;
        }
        if !decorated_plain.contains(&self.body.content) {
            return ChatTrustLevel::Modified;
        }
        ChatTrustLevel::Secure
    }
}

/// `ChatTrustLevel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatTrustLevel {
    Secure,
    Modified,
    NotSecure,
}

impl ChatTrustLevel {
    /// `createTag` — **`SECURE` gets no tag at all**, which is why the return
    /// is an `Option` and not a fourth "plain" variant.
    pub fn create_tag(self) -> Option<rewo_world::chat::MessageTag> {
        match self {
            Self::Modified => Some(rewo_world::chat::MessageTag::CHAT_MODIFIED),
            Self::NotSecure => Some(rewo_world::chat::MessageTag::CHAT_NOT_SECURE),
            Self::Secure => None,
        }
    }
}

/// What `showMessageToPlayer` decides to put on screen.
///
/// Not `Eq`: the shown content is a component now (M127), and `Nbt` carries
/// floats.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatOutcome {
    /// Nothing is shown — `isFullyFiltered`, or a fully-masked partial.
    Dropped,
    Shown {
        /// The **decorated** component to render — what
        /// `ChatComponent.addPlayerMessage` is handed. A component rather than
        /// a string since M127, because that is what the decoration produces
        /// and what carries its style.
        content: rewo_proto::nbt::Nbt,
        tag: Option<rewo_world::chat::MessageTag>,
    },
}

/// `ChatListener.showMessageToPlayer`, minus the four gates Rewo has no state
/// for (`isBlocked`, `isFriendOnlyRestricted`, `onlyShowSecureChat`, and
/// `chatAbilities().canReceivePlayerMessages()`), each of which can only ever
/// *suppress* a message — so omitting them shows a superset, never a wrong
/// line.
///
/// **The two branches disagree about which content they render, and it is not a
/// slip.** With an empty mask vanilla shows `decoratedMessage`, built from
/// `decoratedContent()` — the unsigned override when there is one. With a
/// non-empty mask it re-decorates `message.signedContent()`. So a server that
/// sends both an unsigned rewrite *and* a filter mask has its rewrite ignored;
/// the mask is applied to what was actually signed, which is the only string
/// its bit indices line up with.
pub fn show_message(
    chat: &PlayerChat,
    received_millis: i64,
    decorate: &dyn Fn(&rewo_proto::nbt::Nbt) -> rewo_proto::nbt::Nbt,
    plain: &dyn Fn(&rewo_proto::nbt::Nbt) -> String,
) -> ChatOutcome {
    // Both closures are the session's: decorating needs the `chat_type`
    // registry and the message's bound, and flattening needs the language
    // table. They are parameters rather than a `&PlaySession` because that type
    // owns a socket and has no test module anywhere in the repo (M71/M97), so
    // anything reachable only through it is untestable by construction.

    // `handlePlayerChatMessage` decorates BEFORE `showMessageToPlayer`, and the
    // decorated component is what the trust level is evaluated against.
    let decorated = decorate(&chat.decorated_content());
    // Vanilla evaluates the trust level first and tests `isFullyFiltered`
    // after; the order is unobservable here (nothing is mutated) and kept so
    // the transcription reads in vanilla's sequence.
    let tag = chat
        .trust_level(received_millis, &plain(&decorated))
        .create_tag();
    if chat.filter_mask.is_fully_filtered() {
        return ChatOutcome::Dropped;
    }
    if chat.filter_mask.is_empty() {
        return ChatOutcome::Shown {
            content: decorated,
            tag,
        };
    }
    // The masked branch re-decorates the **signed** content, so a server that
    // sends both a rewrite and a mask has its rewrite ignored — and the mask is
    // applied to the only string that was actually signed.
    match chat.filter_mask.apply_with_formatting(&chat.body.content) {
        Some(filtered) => ChatOutcome::Shown {
            content: decorate(&filtered),
            tag,
        },
        None => ChatOutcome::Dropped,
    }
}

/// `ClientboundSystemChatPacket` — the component, then the **`overlay` bool**
/// that decides whether it is chat at all.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemChat {
    /// The component, **unflattened** (M125).
    ///
    /// It used to be flattened here, and it cannot be any more: a system chat
    /// line is where a server's translatable components actually arrive —
    /// `multiplayer.player.joined`, every death message, every command's
    /// feedback — and resolving one needs the language table, which the wire
    /// has no access to. So the packet carries what the wire said and the
    /// session resolves it, the same split `hud_state` already uses for the
    /// title and action bar.
    pub content: rewo_proto::nbt::Nbt,
    /// `true` routes to `handleOverlay` — the action bar above the hotbar —
    /// and the message never reaches the chat log.
    pub overlay: bool,
}

impl SystemChat {
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let content = r.nbt()?;
        let overlay = r.bool()?;
        Ok(Self { content, overlay })
    }
}

/// `ClientboundDeleteChatPacket` — one packed signature and nothing else.
pub fn read_delete_chat(r: &mut PacketReader<'_>) -> Result<PackedSignature> {
    read_packed_signature(r)
}

/// One thing the chat HUD must do, as decided at the wire.
///
/// The session cannot do any of it itself: adding a message needs the font to
/// wrap with and the GUI tick to stamp, and both live in the app. So the three
/// verbs are queued here and applied there — the same shape `SessionState`
/// already uses for `disguised_chat`.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatEvent {
    /// `ChatComponent.addPlayerMessage` / `addServerSystemMessage`.
    Message {
        /// The message's spans. A `String` until M126b — see
        /// [`rewo_world::chat::GuiMessage::content`]. Player chat arrives as a
        /// plain string on the wire and becomes spans through
        /// [`rewo_world::chat_style::parse_legacy`], which is what resolves the
        /// `§` codes servers put in it; system and disguised chat arrive as
        /// components and go through `parse_component`.
        text: rewo_world::chat_style::ChatLine,
        /// Present only for a signed player message — the key a later
        /// [`Self::Delete`] matches on.
        signature: Option<Box<Signature>>,
        tag: Option<rewo_world::chat::MessageTag>,
        source: rewo_world::chat::MessageSource,
    },
    /// `Gui.setOverlayMessage` — the action bar. **Not a chat line**; a
    /// `system_chat` with `overlay` set never reaches the chat store at all.
    Overlay(String),
    /// `ChatComponent.deleteMessage`, with the signature already resolved
    /// against the cache.
    Delete(Box<Signature>),
}

/// Apply a batch of events to the chat store, returning the last overlay
/// message if any arrived.
///
/// **A free function rather than a method on `PlaySession` on purpose.** The
/// session owns a socket and has no test module anywhere in the repo (M71's
/// finding), so a loop living there is untestable — and a mutation that made
/// it drain nothing survived the whole suite, which is the M86 failure mode in
/// miniature: chat would render empty forever and every test would pass. The
/// session keeps a five-line adapter; this keeps the rule.
///
/// `ChatComponent.tick` runs **after** the batch, so a deletion queued by this
/// batch is not also retried by it. Vanilla separates them for the same
/// reason: `processMessageDeletionQueue` is called from `Gui.tick`, not from
/// the packet handler.
pub fn apply_chat_events(
    chat: &mut rewo_world::chat::ChatComponent,
    events: Vec<ChatEvent>,
    gui_tick: i32,
    ctx: &rewo_world::chat::WrapContext<'_>,
) -> Option<String> {
    let mut overlay = None;
    for event in events {
        match event {
            ChatEvent::Message {
                text,
                signature,
                tag,
                source,
            } => chat.add_message(
                rewo_world::chat::GuiMessage {
                    added_time: gui_tick,
                    content: text,
                    signature,
                    source,
                    tag,
                },
                ctx,
            ),
            // Last one wins: `setOverlayMessage` assigns, and two action-bar
            // messages in one batch cannot both be on screen.
            ChatEvent::Overlay(text) => overlay = Some(text),
            ChatEvent::Delete(sig) => chat.delete_message(&sig, gui_tick, ctx),
        }
    }
    chat.tick(gui_tick, ctx);
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(byte: u8) -> Box<Signature> {
        Box::new([byte; SIGNATURE_BYTES])
    }

    fn varint(v: i32, out: &mut Vec<u8>) {
        let mut v = v as u32;
        loop {
            if v & !0x7f == 0 {
                out.push(v as u8);
                return;
            }
            out.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
    }

    // ── packed signatures ────────────────────────────────────────────────

    #[test]
    fn wire_zero_means_an_inline_signature_not_slot_zero() {
        // The `- 1`. Reading this raw makes wire 0 "cache slot 0" and then
        // reads the 256 inline bytes as the next field.
        let mut body = vec![0u8];
        body.extend_from_slice(&[0xAB; SIGNATURE_BYTES]);
        let mut r = PacketReader::new(&body);
        assert_eq!(
            read_packed_signature(&mut r).unwrap(),
            PackedSignature::Full(sig(0xAB)),
        );
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn wire_n_means_cache_slot_n_minus_one_and_consumes_nothing_more() {
        let body = [1u8];
        let mut r = PacketReader::new(&body);
        assert_eq!(
            read_packed_signature(&mut r).unwrap(),
            PackedSignature::Cached(0),
        );
        assert_eq!(r.remaining(), 0);
        let body = [5u8];
        let mut r = PacketReader::new(&body);
        assert_eq!(
            read_packed_signature(&mut r).unwrap(),
            PackedSignature::Cached(4),
        );
    }

    // ── the cache ────────────────────────────────────────────────────────

    #[test]
    fn the_newest_arrival_lands_in_slot_zero() {
        // The queue is `lastSeen ++ [signature]` and the walk takes
        // `removeLast()`, so the message's own signature is first out.
        let mut c = MessageSignatureCache::new(4);
        c.push(&[sig(1), sig(2)], Some(&sig(3)));
        assert_eq!(c.pack(&sig(3)), Some(0));
        assert_eq!(c.pack(&sig(2)), Some(1));
        assert_eq!(c.pack(&sig(1)), Some(2));
    }

    #[test]
    fn an_existing_entry_slides_down_one_slot() {
        let mut c = MessageSignatureCache::new(4);
        c.push(&[], Some(&sig(1)));
        assert_eq!(c.pack(&sig(1)), Some(0));
        c.push(&[], Some(&sig(2)));
        assert_eq!(c.pack(&sig(2)), Some(0));
        assert_eq!(c.pack(&sig(1)), Some(1));
    }

    #[test]
    fn a_repeated_signature_is_moved_to_the_front_not_duplicated() {
        // The `newEntries` set: the displaced copy is dropped rather than
        // pushed back, so everything below it moves up. A ring buffer would
        // hold two copies and leave the older one indexable.
        let mut c = MessageSignatureCache::new(4);
        c.push(&[], Some(&sig(1)));
        c.push(&[], Some(&sig(2)));
        c.push(&[], Some(&sig(3)));
        // 3, 2, 1
        c.push(&[], Some(&sig(2)));
        assert_eq!(c.pack(&sig(2)), Some(0));
        assert_eq!(c.pack(&sig(3)), Some(1));
        assert_eq!(c.pack(&sig(1)), Some(2));
        // Exactly one copy of 2 survives.
        assert_eq!(
            (0..4).filter(|i| c.unpack(*i) == Some(&*sig(2))).count(),
            1,
        );
    }

    #[test]
    fn the_walk_stops_at_the_capacity_and_drops_the_overflow() {
        let mut c = MessageSignatureCache::new(2);
        c.push(&[sig(1), sig(2)], Some(&sig(3)));
        assert_eq!(c.pack(&sig(3)), Some(0));
        assert_eq!(c.pack(&sig(2)), Some(1));
        assert_eq!(c.pack(&sig(1)), None);
    }

    #[test]
    fn an_out_of_range_id_is_none_rather_than_a_panic() {
        // Vanilla indexes the array raw and throws. A hostile server must not
        // be able to do that here.
        //
        // **The cache is populated on purpose.** An empty one answers `None`
        // for every id, so it cannot distinguish a bounds check from its
        // absence — the first version of this test used an empty cache and a
        // mutation to `id.abs()` survived it. With slot 1 filled, dropping the
        // low-side check turns `unpack(-1)` into `unpack(1)` and hands back a
        // real signature.
        let mut c = MessageSignatureCache::new(4);
        c.push(&[sig(1)], Some(&sig(2)));
        assert_eq!(c.unpack(0), Some(&*sig(2)));
        assert_eq!(c.unpack(1), Some(&*sig(1)));
        assert_eq!(c.unpack(4), None);
        assert_eq!(c.unpack(1_000_000), None);
        assert_eq!(c.unpack(-1), None);
        assert_eq!(c.unpack(-2), None);
        assert_eq!(c.unpack(i32::MIN), None);
    }

    #[test]
    fn a_negative_cache_id_is_reachable_from_the_wire() {
        // Which is why the low-side check is load-bearing rather than
        // defensive decoration: `varint()` returns an i32, a five-byte varint
        // decodes negative, and `id = that - 1` lands in `Cached`.
        let body = [0xFBu8, 0xFF, 0xFF, 0xFF, 0x0F]; // -5
        let mut r = PacketReader::new(&body);
        assert_eq!(
            read_packed_signature(&mut r).unwrap(),
            PackedSignature::Cached(-6),
        );
        let mut c = MessageSignatureCache::new(4);
        c.push(&[], Some(&sig(7)));
        assert_eq!(c.resolve(&PackedSignature::Cached(-6)), None);
    }

    #[test]
    fn an_empty_slot_resolves_to_none() {
        let c = MessageSignatureCache::new(4);
        assert_eq!(c.resolve(&PackedSignature::Cached(0)), None);
    }

    #[test]
    fn a_full_packed_signature_resolves_without_the_cache() {
        let c = MessageSignatureCache::new(4);
        assert_eq!(
            c.resolve(&PackedSignature::Full(sig(9))),
            Some(sig(9)),
        );
    }

    // ── filter mask ──────────────────────────────────────────────────────

    #[test]
    fn an_unknown_filter_mask_type_is_an_error() {
        // `readEnum` indexes `getEnumConstants()`, so vanilla throws. M65's
        // pair of conventions: this is the strict one.
        let body = [3u8];
        assert!(FilterMask::read(&mut PacketReader::new(&body)).is_err());
    }

    #[test]
    fn is_empty_means_pass_through_not_masks_nothing() {
        assert!(FilterMask::PassThrough.is_empty());
        // All bits clear, and still not "empty" — which is what makes vanilla
        // render the signed content instead of the unsigned override.
        assert!(!FilterMask::PartiallyFiltered(vec![0]).is_empty());
    }

    /// The plain text of a mask's component form, for the tests that only
    /// care which characters survived.
    fn masked_text(m: &FilterMask, text: &str) -> Option<String> {
        m.apply_with_formatting(text).map(|c| {
            rewo_world::chat_style::plain_text(&rewo_world::chat_style::parse_component(
                &c,
                rewo_world::chat_style::ChatStyle::WHITE,
                None,
            ))
        })
    }

    #[test]
    fn a_partial_mask_hashes_the_set_bits_only() {
        let m = FilterMask::PartiallyFiltered(vec![0b0110]);
        assert_eq!(masked_text(&m, "abcd").as_deref(), Some("a##d"));
    }

    #[test]
    fn a_fully_filtered_message_is_shown_as_nothing() {
        assert_eq!(FilterMask::FullyFiltered.apply_with_formatting("abcd"), None);
    }

    #[test]
    fn mask_bits_past_the_end_of_the_text_are_ignored() {
        let m = FilterMask::PartiallyFiltered(vec![-1i64]);
        assert_eq!(masked_text(&m, "ab").as_deref(), Some("##"));
    }

    /// The client calls `applyWithFormatting`, not `apply`, and the difference
    /// is visible: `FILTERED_STYLE` is `withColor(DARK_GRAY)`, so the hashes
    /// are grey against the message's own colour. A string of `#`s takes
    /// whatever colour surrounds it, which is what Rewo drew before M127.
    #[test]
    fn a_filtered_run_is_dark_gray_and_an_unfiltered_one_inherits() {
        let m = FilterMask::PartiallyFiltered(vec![0b0110]);
        let spans = rewo_world::chat_style::parse_component(
            &m.apply_with_formatting("abcd").unwrap(),
            rewo_world::chat_style::ChatStyle::plain([1.0, 0.0, 0.0]),
            None,
        );
        let dark_gray = rewo_world::chat_style::parse_color("dark_gray").unwrap();
        let runs: Vec<(&str, [f32; 3])> =
            spans.iter().map(|s| (s.text.as_str(), s.color)).collect();
        assert_eq!(
            runs,
            vec![
                ("a", [1.0, 0.0, 0.0]),
                ("##", dark_gray),
                ("d", [1.0, 0.0, 0.0]),
            ],
            "the hashes are grey; the surviving text keeps the parent colour"
        );
    }

    /// `applyWithFormatting` walks `BitSet.nextSetBit` against
    /// `String.substring`, i.e. **UTF-16 code units**. The replaced `apply`
    /// walked `str::chars()`, so every bit past the first astral character
    /// addressed the wrong one.
    ///
    /// MUTATION: index by scalar values (`chars().enumerate()`). Under that
    /// reading bit 2 lands on the `b` and the emoji survives, which is the
    /// exact inverse of this assertion.
    #[test]
    fn the_mask_indexes_utf16_code_units_not_scalars() {
        // "a" is unit 0; the emoji occupies units 1 and 2; "b" is unit 3.
        let m = FilterMask::PartiallyFiltered(vec![0b0110]);
        assert_eq!(
            masked_text(&m, "a\u{1F600}b").as_deref(),
            Some("a##b"),
            "the astral character is masked, and by TWO hashes"
        );
        // And the bit that a scalar-indexed reading would have used for `b`.
        let m = FilterMask::PartiallyFiltered(vec![0b1000]);
        assert_eq!(masked_text(&m, "a\u{1F600}b").as_deref(), Some("a\u{1F600}#"));
    }

    // ── trust level ──────────────────────────────────────────────────────

    /// A component's plain text, as the session's flattener would produce it.
    fn plain(tag: &rewo_proto::nbt::Nbt) -> String {
        rewo_world::chat_style::plain_text(&rewo_world::chat_style::parse_component(
            tag,
            rewo_world::chat_style::ChatStyle::WHITE,
            None,
        ))
    }

    /// A session with no `chat_type` registry decorates nothing — the fallback
    /// `decorate_chat` takes, and the pre-M127 behaviour.
    fn undecorated(content: &rewo_proto::nbt::Nbt) -> rewo_proto::nbt::Nbt {
        content.clone()
    }

    fn shown(c: &PlayerChat, received: i64) -> ChatOutcome {
        show_message(c, received, &undecorated, &plain)
    }

    fn chat(signature: Option<Box<Signature>>, content: &str, unsigned: Option<&str>) -> PlayerChat {
        PlayerChat {
            global_index: 0,
            sender: 0,
            index: 0,
            signature,
            body: SignedMessageBody {
                content: content.into(),
                timestamp_millis: 0,
                salt: 0,
                last_seen: Vec::new(),
            },
            unsigned_content: unsigned.map(|u| rewo_proto::nbt::Nbt::String(u.to_string())),
            filter_mask: FilterMask::PassThrough,
            bound: crate::session::ChatTypeBound {
                chat_type: crate::session::ChatTypeRef::Registry(0),
                name: rewo_proto::nbt::Nbt::String("Steve".into()),
                target_name: None,
            },
        }
    }

    #[test]
    fn an_unsigned_message_is_not_secure() {
        assert_eq!(
            chat(None, "hi", None).trust_level(0, "hi"),
            ChatTrustLevel::NotSecure,
        );
    }

    #[test]
    fn an_expired_message_is_not_secure_however_well_signed() {
        let c = chat(Some(sig(1)), "hi", None);
        assert_eq!(
            c.trust_level(MESSAGE_EXPIRES_AFTER_CLIENT_MILLIS, "hi"),
            ChatTrustLevel::Secure,
        );
        assert_eq!(
            c.trust_level(MESSAGE_EXPIRES_AFTER_CLIENT_MILLIS + 1, "hi"),
            ChatTrustLevel::NotSecure,
        );
    }

    #[test]
    fn the_expiry_test_is_one_sided() {
        // `now.isAfter(stamp + window)` — a message stamped in the future is
        // not expired, it is merely early.
        let c = chat(Some(sig(1)), "hi", None);
        assert_eq!(c.trust_level(-1_000_000, "hi"), ChatTrustLevel::Secure);
    }

    #[test]
    fn an_unsigned_override_containing_the_signed_text_is_secure() {
        // `isModified` is `!decorated.contains(signedContent)` — a server that
        // merely *decorates* has not modified.
        assert_eq!(
            chat(Some(sig(1)), "hi", Some("[VIP] hi")).trust_level(0, "[VIP] hi"),
            ChatTrustLevel::Secure,
        );
    }

    #[test]
    fn an_unsigned_override_that_replaces_the_text_is_modified() {
        assert_eq!(
            chat(Some(sig(1)), "hi", Some("goodbye")).trust_level(0, "goodbye"),
            ChatTrustLevel::Modified,
        );
    }

    #[test]
    fn only_secure_has_no_tag() {
        assert_eq!(ChatTrustLevel::Secure.create_tag(), None);
        assert_eq!(
            ChatTrustLevel::NotSecure.create_tag(),
            Some(rewo_world::chat::MessageTag::CHAT_NOT_SECURE),
        );
        assert_eq!(
            ChatTrustLevel::Modified.create_tag(),
            Some(rewo_world::chat::MessageTag::CHAT_MODIFIED),
        );
    }

    // ── show_message ─────────────────────────────────────────────────────

    #[test]
    fn an_empty_mask_shows_the_unsigned_override() {
        let c = chat(Some(sig(1)), "hi", Some("[VIP] hi"));
        let ChatOutcome::Shown { content, tag } = shown(&c, 0) else {
            panic!("expected a shown message")
        };
        assert_eq!(plain(&content), "[VIP] hi");
        assert_eq!(tag, None);
    }

    #[test]
    fn a_non_empty_mask_shows_the_signed_content_instead() {
        // The asymmetry: the mask's bit indices only line up with the string
        // that was signed, so the server's rewrite is discarded.
        //
        // The tag is `Modified` and that is not incidental — see
        // `the_tag_is_judged_on_the_message_not_on_what_is_rendered`.
        let mut c = chat(Some(sig(1)), "hello", Some("REWRITTEN"));
        c.filter_mask = FilterMask::PartiallyFiltered(vec![0b1]);
        let ChatOutcome::Shown { content, tag } = shown(&c, 0) else {
            panic!("expected a shown message")
        };
        assert_eq!(plain(&content), "#ello");
        assert_eq!(tag, Some(rewo_world::chat::MessageTag::CHAT_MODIFIED));
    }

    #[test]
    fn the_tag_is_judged_on_the_message_not_on_what_is_rendered() {
        // `evaluateTrustLevel` runs on the whole `PlayerChatMessage`, before
        // and independently of the mask branch that decides which string to
        // draw. So a masked message from a server that also rewrote it is
        // rendered from the SIGNED content — the rewrite discarded — and still
        // flagged `Modified` **because of** the rewrite the reader never sees.
        // Judging the tag from the rendered text instead would call it Secure,
        // which is the more intuitive reading and would hide the rewrite.
        let mut c = chat(Some(sig(1)), "hello", Some("REWRITTEN"));
        c.filter_mask = FilterMask::PartiallyFiltered(vec![0b1]);
        let ChatOutcome::Shown { content, tag } = shown(&c, 0) else {
            panic!("expected a shown message");
        };
        let text = plain(&content);
        assert!(text.contains("ello"), "rendered from the signed content");
        assert!(!text.contains("REWRITTEN"));
        assert_eq!(tag, Some(rewo_world::chat::MessageTag::CHAT_MODIFIED));
    }

    #[test]
    fn an_all_clear_partial_mask_still_suppresses_the_override() {
        // Because `isEmpty()` is `PASS_THROUGH`, not "no bits set". Compare
        // with `an_empty_mask_shows_the_unsigned_override`, which is the same
        // message with a `PassThrough` mask and renders "[VIP] hi".
        let mut c = chat(Some(sig(1)), "hello", Some("REWRITTEN"));
        c.filter_mask = FilterMask::PartiallyFiltered(vec![0]);
        let ChatOutcome::Shown { content, tag } = shown(&c, 0) else {
            panic!("expected a shown message")
        };
        assert_eq!(plain(&content), "hello");
        assert_eq!(tag, Some(rewo_world::chat::MessageTag::CHAT_MODIFIED));
    }

    #[test]
    fn a_fully_filtered_message_is_dropped_before_the_tag_is_computed() {
        let mut c = chat(None, "hello", None);
        c.filter_mask = FilterMask::FullyFiltered;
        assert_eq!(shown(&c, 0), ChatOutcome::Dropped);
    }

    // ── the decoration, from `show_message`'s side ───────────────────────

    /// A decorator that wraps the content the way `chat.type.text` does. Not
    /// the real one — this file has no registry — but the same shape: a
    /// component whose text contains the content.
    fn angle_brackets(content: &rewo_proto::nbt::Nbt) -> rewo_proto::nbt::Nbt {
        rewo_proto::nbt::Nbt::String(format!("<Steve> {}", plain(content)))
    }

    #[test]
    fn the_shown_content_is_the_decorated_component() {
        let c = chat(Some(sig(1)), "hi", None);
        let ChatOutcome::Shown { content, .. } = show_message(&c, 0, &angle_brackets, &plain)
        else {
            panic!("expected a shown message")
        };
        assert_eq!(plain(&content), "<Steve> hi");
    }

    /// **The masked branch re-decorates the filtered content**, so the
    /// decoration is applied twice over in neither branch and exactly once in
    /// both. A reading that decorated only the unmasked branch would render a
    /// filtered message with no sender.
    #[test]
    fn the_masked_branch_decorates_too() {
        let mut c = chat(Some(sig(1)), "hello", None);
        c.filter_mask = FilterMask::PartiallyFiltered(vec![0b1]);
        let ChatOutcome::Shown { content, .. } = show_message(&c, 0, &angle_brackets, &plain)
        else {
            panic!("expected a shown message")
        };
        assert_eq!(plain(&content), "<Steve> #ello");
    }

    /// `evaluateTrustLevel` is handed the **decorated** message, so
    /// `isModified` asks whether `<Steve> hello` contains `hello` — not
    /// whether `hello` does.
    ///
    /// The two readings agree for every ordinary message, because a decoration
    /// only wraps text around the content. They diverge exactly when the
    /// decoration DROPS it: a translation key the table does not carry, or a
    /// server-defined chat type whose parameters omit `content`.
    ///
    /// MUTATION: pass the undecorated content to `trust_level` (the pre-M127
    /// behaviour). Both fixtures below then read Secure.
    #[test]
    fn a_decoration_that_drops_the_content_is_modified() {
        let c = chat(Some(sig(1)), "hello", None);
        // A decoration that keeps it.
        assert_eq!(
            c.trust_level(0, &plain(&angle_brackets(&c.decorated_content()))),
            ChatTrustLevel::Secure,
        );
        // One that does not — `chat.type.emote` with the content parameter
        // omitted, which a datapack may legally define.
        assert_eq!(
            c.trust_level(0, "* Steve waves"),
            ChatTrustLevel::Modified,
        );
    }

    // ── the packet walks ─────────────────────────────────────────────────

    #[test]
    fn system_chat_carries_the_overlay_flag() {
        // A bare NBT string tag (id 8) then the bool. `overlay` routes the
        // message to the action bar instead of the chat log, and reading the
        // component alone loses that entirely.
        let mut body = vec![8u8, 0, 2, b'h', b'i'];
        body.push(1);
        let got = SystemChat::read(&mut PacketReader::new(&body)).unwrap();
        assert!(got.overlay);
        let mut body = vec![8u8, 0, 2, b'h', b'i'];
        body.push(0);
        assert!(!SystemChat::read(&mut PacketReader::new(&body)).unwrap().overlay);
    }

    #[test]
    fn player_chat_walks_its_whole_body() {
        // The point of the test is the reader position: a prefix parse leaves
        // bytes behind and cannot notice a field it mis-sized.
        let mut body = Vec::new();
        varint(7, &mut body); // globalIndex
        body.extend_from_slice(&[0u8; 16]); // sender
        varint(3, &mut body); // index
        body.push(1); // signature present
        body.extend_from_slice(&[0x11; SIGNATURE_BYTES]);
        // SignedMessageBody.Packed
        varint(2, &mut body);
        body.extend_from_slice(b"hi");
        body.extend_from_slice(&1234i64.to_be_bytes()); // timestamp
        body.extend_from_slice(&9i64.to_be_bytes()); // salt
        varint(1, &mut body); // lastSeen count
        varint(1, &mut body); // -> cached slot 0
        body.push(0); // unsignedContent absent
        varint(0, &mut body); // FilterMask PASS_THROUGH
        // ChatType.Bound: holder id 1 (-> chat type 0), name, no target
        varint(1, &mut body);
        body.extend_from_slice(&[8u8, 0, 5, b'S', b't', b'e', b'v', b'e']);
        body.push(0);

        let mut r = PacketReader::new(&body);
        let got = PlayerChat::read(&mut r).unwrap();
        assert_eq!(r.remaining(), 0, "the walk must consume the whole body");
        assert_eq!(got.global_index, 7);
        assert_eq!(got.index, 3);
        assert_eq!(got.signature, Some(sig(0x11)));
        assert_eq!(got.body.content, "hi");
        assert_eq!(got.body.timestamp_millis, 1234);
        assert_eq!(got.body.salt, 9);
        assert_eq!(got.body.last_seen, vec![PackedSignature::Cached(0)]);
        assert_eq!(got.unsigned_content, None);
        assert_eq!(got.filter_mask, FilterMask::PassThrough);
        assert_eq!(
            got.bound.name,
            rewo_proto::nbt::Nbt::String("Steve".into())
        );
        assert_eq!(
            got.bound.chat_type,
            crate::session::ChatTypeRef::Registry(0)
        );
    }

    #[test]
    fn a_last_seen_list_over_the_cap_is_rejected() {
        // `readCollection(limitValue(ArrayList::new, 20), …)`.
        //
        // **The body is well-formed and complete.** A truncated one errors with
        // or without the cap — only the error's *shape* would differ — so a
        // fixture that stops after the count witnesses nothing. This one
        // carries 21 perfectly readable entries, which parse cleanly if the
        // cap is dropped. The error variant is asserted for the same reason.
        let mut body = Vec::new();
        varint(2, &mut body);
        body.extend_from_slice(b"hi");
        body.extend_from_slice(&0i64.to_be_bytes());
        body.extend_from_slice(&0i64.to_be_bytes());
        varint(LAST_SEEN_MAX as i32 + 1, &mut body);
        for i in 0..=LAST_SEEN_MAX as i32 {
            varint(i + 1, &mut body); // a cached id, one byte each
        }
        let err = SignedMessageBody::read(&mut PacketReader::new(&body)).unwrap_err();
        assert!(
            matches!(
                err,
                rewo_proto::ProtoError::LengthOutOfRange {
                    what: "lastSeen",
                    ..
                }
            ),
            "expected the cap to reject it, got {err:?}",
        );

        // …and exactly at the cap it is accepted, so the bound is `<= 20` and
        // not `< 20`.
        let mut body = Vec::new();
        varint(2, &mut body);
        body.extend_from_slice(b"hi");
        body.extend_from_slice(&0i64.to_be_bytes());
        body.extend_from_slice(&0i64.to_be_bytes());
        varint(LAST_SEEN_MAX as i32, &mut body);
        for i in 0..LAST_SEEN_MAX as i32 {
            varint(i + 1, &mut body);
        }
        let got = SignedMessageBody::read(&mut PacketReader::new(&body)).unwrap();
        assert_eq!(got.last_seen.len(), LAST_SEEN_MAX);
    }

    // ── applying a batch ─────────────────────────────────────────────────

    fn wrap_ctx() -> rewo_world::chat::WrapContext<'static> {
        fn w6(s: &str, style: rewo_world::chat_style::ChatStyle) -> i32 {
            s.chars().count() as i32 * (6 + i32::from(style.bold))
        }
        rewo_world::chat::WrapContext {
            options: rewo_world::chat::ChatOptions::default(),
            focused: false,
            width_of: &w6,
            deleted_marker_text: "deleted",
        }
    }

    fn message(text: &str, signature: Option<Box<Signature>>) -> ChatEvent {
        ChatEvent::Message {
            text: vec![rewo_world::chat_style::ChatStyle::WHITE.span(text)],
            signature,
            tag: None,
            source: rewo_world::chat::MessageSource::Player,
        }
    }

    #[test]
    fn a_batch_of_messages_reaches_the_store() {
        // The witness that did not exist when a mutation made the drain a
        // no-op: chat would have rendered empty forever with every test green.
        let mut chat = rewo_world::chat::ChatComponent::new();
        let overlay = apply_chat_events(
            &mut chat,
            vec![message("first", None), message("second", None)],
            7,
            &wrap_ctx(),
        );
        assert_eq!(overlay, None);
        assert_eq!(chat.all_messages().len(), 2);
        assert_eq!(
            rewo_world::chat_style::plain_text(&chat.trimmed_messages()[0].text),
            "second"
        );
        assert_eq!(chat.trimmed_messages()[0].added_time, 7);
    }

    #[test]
    fn an_overlay_is_returned_and_never_stored_as_chat() {
        let mut chat = rewo_world::chat::ChatComponent::new();
        let overlay = apply_chat_events(
            &mut chat,
            vec![ChatEvent::Overlay("action bar".into())],
            0,
            &wrap_ctx(),
        );
        assert_eq!(overlay.as_deref(), Some("action bar"));
        assert!(chat.all_messages().is_empty());
    }

    #[test]
    fn the_last_overlay_of_a_batch_wins() {
        let mut chat = rewo_world::chat::ChatComponent::new();
        let overlay = apply_chat_events(
            &mut chat,
            vec![
                ChatEvent::Overlay("first".into()),
                ChatEvent::Overlay("second".into()),
            ],
            0,
            &wrap_ctx(),
        );
        assert_eq!(overlay.as_deref(), Some("second"));
    }

    #[test]
    fn a_delete_in_the_same_batch_is_queued_not_applied_twice() {
        // The message is 0 ticks old, so the deletion is delayed rather than
        // applied — and `tick` running after the batch must not immediately
        // retry it, or the 60-tick guard is defeated by its own batch.
        let mut chat = rewo_world::chat::ChatComponent::new();
        apply_chat_events(
            &mut chat,
            vec![
                message("secret", Some(sig(4))),
                ChatEvent::Delete(sig(4)),
            ],
            0,
            &wrap_ctx(),
        );
        assert_eq!(
            rewo_world::chat_style::plain_text(&chat.all_messages()[0].content),
            "secret"
        );
        assert_eq!(chat.deletion_queue_len(), 1);
        // …and it lands once the message is old enough.
        apply_chat_events(&mut chat, Vec::new(), 60, &wrap_ctx());
        assert_eq!(
            rewo_world::chat_style::plain_text(&chat.all_messages()[0].content),
            "deleted"
        );
        assert_eq!(chat.deletion_queue_len(), 0);
    }

    #[test]
    fn an_empty_batch_still_ticks_the_deletion_queue() {
        // `tick` is outside the loop, so a frame with no packets still
        // advances a pending deletion. Inside the loop it would stall forever
        // on a quiet server.
        let mut chat = rewo_world::chat::ChatComponent::new();
        apply_chat_events(
            &mut chat,
            vec![message("secret", Some(sig(4))), ChatEvent::Delete(sig(4))],
            0,
            &wrap_ctx(),
        );
        apply_chat_events(&mut chat, Vec::new(), 100, &wrap_ctx());
        assert_eq!(
            rewo_world::chat_style::plain_text(&chat.all_messages()[0].content),
            "deleted"
        );
    }

    #[test]
    fn delete_chat_is_one_packed_signature() {
        let body = [3u8];
        let mut r = PacketReader::new(&body);
        assert_eq!(
            read_delete_chat(&mut r).unwrap(),
            PackedSignature::Cached(2),
        );
        assert_eq!(r.remaining(), 0);
    }
    // -- M126a: moved here from `chat_translate`'s test module ------------
    //
    // These two drive `SystemChat::read` over real bytes, so they belong on
    // the wire side; `chat_translate` moved down to `rewo-world` and cannot
    // name a packet. Nothing about what they assert changed.

    fn lang(pairs: &[(&str, &str)]) -> rewo_data::lang::Language {
        rewo_data::lang::Language::from_map(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        )
    }

    // -- the chain a `system_chat` actually walks -------------------------

    /// A `system_chat` body, byte for byte, carrying a translatable component.
    ///
    /// Written out rather than built from an `Nbt` so the test drives the real
    /// `SystemChat::read` over real bytes — M92's rule: a witness that hands
    /// production the value production is supposed to derive is grading
    /// itself.
    fn system_chat_body(key: &str, arg: &str, overlay: bool) -> Vec<u8> {
        fn string_field(out: &mut Vec<u8>, name: &str, value: &str) {
            out.push(0x08); // TAG_String
            out.extend_from_slice(&(name.len() as u16).to_be_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&(value.len() as u16).to_be_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        let mut out = vec![0x0a]; // TAG_Compound, unnamed root
        string_field(&mut out, "translate", key);
        // "with": TAG_List of TAG_String, one element.
        out.push(0x09);
        out.extend_from_slice(&4u16.to_be_bytes());
        out.extend_from_slice(b"with");
        out.push(0x08); // element type
        out.extend_from_slice(&1i32.to_be_bytes());
        out.extend_from_slice(&(arg.len() as u16).to_be_bytes());
        out.extend_from_slice(arg.as_bytes());
        out.push(0x00); // end of compound
        out.push(u8::from(overlay));
        out
    }

    /// The whole chain: wire bytes -> `SystemChat::read` -> the flatten the
    /// session performs. This is the join `PlaySession` makes, minus the one
    /// field read that hands over the table.
    #[test]
    fn a_system_chat_bodys_translatable_resolves_through_the_flatten() {
        let body = system_chat_body("multiplayer.player.joined", "Steve", false);
        let packet =
            SystemChat::read(&mut rewo_proto::reader::PacketReader::new(&body))
                .unwrap();
        assert!(!packet.overlay);
        let l = lang(&[("multiplayer.player.joined", "%s joined the game")]);
        assert_eq!(
            rewo_world::chat_translate::chat_component_text(&packet.content, Some(&l)),
            "Steve joined the game"
        );
    }

    /// The same bytes with no table: the key, which is what every Rewo session
    /// put on screen before M125. Kept as a test so "passing `None`" stays a
    /// defined behaviour rather than an accident.
    #[test]
    fn the_same_body_with_no_table_is_the_pre_m125_rendering() {
        let body = system_chat_body("multiplayer.player.joined", "Steve", false);
        let packet =
            SystemChat::read(&mut rewo_proto::reader::PacketReader::new(&body))
                .unwrap();
        assert_eq!(
            rewo_world::chat_translate::chat_component_text(&packet.content, None),
            "multiplayer.player.joined"
        );
    }
}
