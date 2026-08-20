//! The two remaining chat packets, and the client state they feed (M114).
//!
//! `ClientboundCommandSuggestionsPacket` is the autocomplete reply;
//! `ClientboundCustomChatCompletionsPacket` is the server's own word list
//! (plugin names, nicknames) merged into the same popup. Both are plain
//! decodes; what is not plain is the state around them, which is why the
//! `ClientSuggestionProvider` half lives here beside them rather than in the
//! UI — the same placement M108 gave the `MessageSignatureCache`.
//!
//! # `toSuggestions` uses the CONSTRUCTOR, not `Suggestions.create`
//!
//! ```java
//! public Suggestions toSuggestions() {
//!    StringRange range = StringRange.between(this.start, this.start + this.length);
//!    return new Suggestions(range, this.suggestions.stream().map(...).toList());
//! }
//! ```
//!
//! So a reply is **not** deduped, **not** re-expanded and **not** sorted — the
//! server's order is displayed verbatim. It looks sorted because the server
//! built its list through `Suggestions.create` before sending, but a plugin
//! that hand-builds one gets its order honoured, and a duplicated entry is
//! shown twice. Routing the reply through [`Suggestions::create`] would be the
//! natural-looking mistake and would silently re-order every server's
//! autocomplete.
//!
//! Note also that the range comes from the packet's `start`/`length` and is
//! applied to **every** entry, so the client never decides which span a reply
//! replaces — the server does.
//!
//! # One pending request, not a queue
//!
//! `ClientSuggestionProvider.customSuggestion` **cancels** the outstanding
//! future, bumps a counter, and sends; `completeCustomSuggestions` accepts a
//! reply only when `id == pendingSuggestionsId` and then resets that id to
//! `-1`. Three consequences, and each is a behaviour rather than an
//! optimisation:
//!
//! * a reply to a superseded request is **dropped**, which is what stops a
//!   slow server repainting the popup with the answer to a prefix you have
//!   already typed past;
//! * a **second** reply carrying the same id is dropped too, because the id
//!   was reset by the first;
//! * the counter starts at `-1` and pre-increments, so the first request on a
//!   connection is id **0**.
//!
//! **One deliberate divergence.** `-1` is both the idle sentinel and a legal
//! VarInt, so a server sending `command_suggestions` with id `-1` while
//! nothing is outstanding satisfies vanilla's `id == pendingSuggestionsId` and
//! then dereferences a null future — vanilla throws. Rewo has no pending
//! request to complete and drops it. The state here is modelled as an
//! `Option`, so the sentinel is not representable and the crash is not
//! reachable.

use rewo_proto::reader::PacketReader;
use rewo_proto::{ProtoError, Result};
use rewo_world::suggestions::{StringRange, Suggestion, Suggestions};
use std::collections::BTreeSet;

/// `ClientboundCommandSuggestionsPacket`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSuggestionsReply {
    /// Matched against the id of the request that is still outstanding.
    pub id: i32,
    /// The span of the sent command this reply replaces.
    pub start: i32,
    pub length: i32,
    /// `(text, tooltip)`. The tooltip is a `TRUSTED_OPTIONAL_STREAM_CODEC`
    /// component, flattened on arrival like every other component Rewo reads.
    pub entries: Vec<(String, Option<String>)>,
}

impl CommandSuggestionsReply {
    pub fn read(body: &[u8]) -> Result<Self> {
        let mut r = PacketReader::new(body);
        let id = r.varint()?;
        let start = r.varint()?;
        let length = r.varint()?;
        // Each entry is at least a length byte plus the optional's boolean.
        let count = r.count("command suggestions", 2)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let text = r.string(32767)?;
            // M163 left this flattened: it is the SMALLEST third of that gap.
            // Nothing renders `Suggestion::tooltip`, and the local suggester
            // throws away the six selector keys it already holds. See the table
            // on `component_wire::nbt_text`.
            let tooltip = r.option(|r| Ok(r.nbt()?.to_plain_text()))?;
            entries.push((text, tooltip));
        }
        Ok(Self {
            id,
            start,
            length,
            entries,
        })
    }

    /// `ClientboundCommandSuggestionsPacket.toSuggestions`.
    ///
    /// Verbatim order, no dedupe, one shared range — see the module docs.
    ///
    /// A negative `start` or `length` cannot come from a vanilla server (both
    /// are derived from a `StringRange` over a real string) but both are
    /// signed VarInts on the wire, so they are clamped to zero here rather
    /// than being allowed to wrap a `usize` into a range that would panic the
    /// first time something sliced with it.
    pub fn to_suggestions(&self) -> Suggestions {
        let start = self.start.max(0) as usize;
        let end = start + self.length.max(0) as usize;
        let range = StringRange::between(start, end);
        Suggestions {
            range,
            list: self
                .entries
                .iter()
                .map(|(text, tooltip)| {
                    Suggestion::new(range, text.clone()).with_tooltip(tooltip.clone())
                })
                .collect(),
        }
    }
}

/// `ClientboundCustomChatCompletionsPacket.Action`.
///
/// `readEnum` indexes `getEnumConstants()`, so an out-of-range ordinal throws
/// — M65's strict convention, not `ByIdMap`'s forgiving one. Getting that
/// backwards here would apply an unknown action as `ADD` and quietly poison
/// the completion set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionAction {
    Add,
    Remove,
    Set,
}

/// `ClientboundCustomChatCompletionsPacket` — an enum ordinal then a counted
/// list of strings.
pub fn read_custom_chat_completions(body: &[u8]) -> Result<(CompletionAction, Vec<String>)> {
    let mut r = PacketReader::new(body);
    let action = match r.varint()? {
        0 => CompletionAction::Add,
        1 => CompletionAction::Remove,
        2 => CompletionAction::Set,
        other => {
            return Err(ProtoError::Frame(format!(
                "custom_chat_completions action ordinal {other} out of range"
            )))
        }
    };
    let count = r.count("custom chat completions", 1)?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        entries.push(r.string(32767)?);
    }
    Ok((action, entries))
}

/// `ServerboundCommandSuggestionPacket`'s cap — **32500**, which is neither
/// `readUtf`'s default 32767 nor the chat screen's 256, and is the only place
/// in the protocol carrying that number.
///
/// It is counted in **UTF-16 units**, because the check is `string.length()`.
pub const COMMAND_SUGGESTION_MAX_LEN: usize = 32500;

/// `ServerboundCommandSuggestionPacket`'s body — a VarInt id then the command.
///
/// Returned as a body rather than a whole packet so it stays reachable from a
/// test: the packet id lives on `PlaySession`, which owns a socket and has no
/// test module (M71/M97). The caller prepends it.
///
/// **A deliberate, unreachable deviation:** `writeUtf(s, max)` *throws* when
/// `s.length() > max`, and this truncates. Nothing can reach either branch —
/// the only caller is the chat field, capped at 256 — so the choice is
/// between a client that dies on an input it cannot receive and one that does
/// not.
pub fn write_command_suggestion(id: i32, command: &str) -> Vec<u8> {
    let mut out = Vec::new();
    rewo_proto::varint::write_varint(&mut out, id);
    let units: Vec<u16> = command.encode_utf16().collect();
    let command = if units.len() > COMMAND_SUGGESTION_MAX_LEN {
        String::from_utf16_lossy(&units[..COMMAND_SUGGESTION_MAX_LEN])
    } else {
        command.to_string()
    };
    rewo_proto::varint::write_varint(&mut out, command.len() as i32);
    out.extend_from_slice(command.as_bytes());
    out
}

/// `ClientSuggestionProvider`'s completion set and pending-request slot.
#[derive(Clone, Debug, Default)]
pub struct SuggestionProviderState {
    /// `customCompletionSuggestions`.
    custom: BTreeSet<String>,
    /// `pendingSuggestionsId` plus `pendingSuggestionsFuture`, as one
    /// `Option` — see the module docs on why the two are not modelled
    /// separately.
    pending: Option<i32>,
    /// The counter behind `++this.pendingSuggestionsId`. Vanilla keeps one
    /// field for both this and the sentinel; splitting them is what makes the
    /// `-1` crash unrepresentable.
    next_id: i32,
}

impl SuggestionProviderState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `modifyCustomCompletions`.
    ///
    /// `SET` **clears then adds**, so it is not `ADD` on an empty set — a
    /// server uses it to replace a list wholesale and reading it as an add
    /// leaves every stale entry in place.
    pub fn apply_completions(&mut self, action: CompletionAction, entries: &[String]) {
        match action {
            CompletionAction::Add => self.custom.extend(entries.iter().cloned()),
            CompletionAction::Remove => {
                for e in entries {
                    self.custom.remove(e);
                }
            }
            CompletionAction::Set => {
                self.custom.clear();
                self.custom.extend(entries.iter().cloned());
            }
        }
    }

    /// `getCustomTabSuggestions`.
    ///
    /// The union of the online players and the custom set, deduped — a name in
    /// both appears once. Vanilla's early return for an empty custom set is an
    /// optimisation, not a different answer, so it is not reproduced.
    ///
    /// Vanilla's is a `HashSet` and therefore unordered; the order does not
    /// escape, because `Suggestions.create` sorts whatever reaches it. It is
    /// deterministic here anyway so that a test can name a list.
    pub fn tab_suggestions<'a>(&self, online: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut out: BTreeSet<String> = online.into_iter().map(str::to_string).collect();
        out.extend(self.custom.iter().cloned());
        out.into_iter().collect()
    }

    pub fn custom_completions(&self) -> impl Iterator<Item = &str> {
        self.custom.iter().map(String::as_str)
    }

    /// `customSuggestion` — cancel whatever was outstanding, take the next id,
    /// and hand back the body to send.
    pub fn begin_request(&mut self, command: &str) -> (i32, Vec<u8>) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending = Some(id);
        (id, write_command_suggestion(id, command))
    }

    pub fn pending_id(&self) -> Option<i32> {
        self.pending
    }

    /// `completeCustomSuggestions` — accept a reply only for the request still
    /// outstanding, and clear the slot so a repeat is ignored.
    pub fn complete(&mut self, reply: &CommandSuggestionsReply) -> Option<Suggestions> {
        if self.pending != Some(reply.id) {
            return None;
        }
        self.pending = None;
        Some(reply.to_suggestions())
    }

    /// Nothing in vanilla clears the set mid-session, but a dimension change
    /// or a respawn rebuilds the provider, so a disconnect must not leave a
    /// previous server's words behind.
    pub fn reset(&mut self) {
        self.custom.clear();
        self.pending = None;
        self.next_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn utf(s: &str, out: &mut Vec<u8>) {
        varint(s.len() as i32, out);
        out.extend_from_slice(s.as_bytes());
    }

    /// An NBT string tag, which is what a flattened `Component` of plain text
    /// decodes from.
    fn nbt_string(s: &str, out: &mut Vec<u8>) {
        out.push(8); // TAG_String
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn reply_bytes(id: i32, start: i32, length: i32, entries: &[(&str, Option<&str>)]) -> Vec<u8> {
        let mut b = Vec::new();
        varint(id, &mut b);
        varint(start, &mut b);
        varint(length, &mut b);
        varint(entries.len() as i32, &mut b);
        for (text, tooltip) in entries {
            utf(text, &mut b);
            match tooltip {
                Some(t) => {
                    b.push(1);
                    nbt_string(t, &mut b);
                }
                None => b.push(0),
            }
        }
        b
    }

    // ── the reply ────────────────────────────────────────────────────────

    #[test]
    fn a_reply_reads_its_range_and_its_entries() {
        let b = reply_bytes(3, 5, 4, &[("give", None), ("gamemode", Some("a tooltip"))]);
        let r = CommandSuggestionsReply::read(&b).unwrap();
        assert_eq!(r.id, 3);
        assert_eq!((r.start, r.length), (5, 4));
        assert_eq!(
            r.entries,
            [
                ("give".to_string(), None),
                ("gamemode".to_string(), Some("a tooltip".to_string())),
            ]
        );
    }

    #[test]
    fn to_suggestions_keeps_the_servers_order_and_its_duplicates() {
        // The finding: `toSuggestions` calls the CONSTRUCTOR, not
        // `Suggestions.create`, so nothing is sorted or deduped. Routing it
        // through `create` — which is what the rest of this subsystem does —
        // would silently re-order every server's autocomplete.
        let b = reply_bytes(0, 0, 0, &[("zeta", None), ("alpha", None), ("zeta", None)]);
        let s = CommandSuggestionsReply::read(&b).unwrap().to_suggestions();
        let texts: Vec<&str> = s.list.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["zeta", "alpha", "zeta"]);
    }

    #[test]
    fn every_entry_shares_the_packets_own_range() {
        // The client never decides which span a reply replaces.
        let b = reply_bytes(0, 5, 4, &[("give", None), ("gamemode", None)]);
        let s = CommandSuggestionsReply::read(&b).unwrap().to_suggestions();
        assert_eq!(s.range, StringRange::between(5, 9));
        assert!(s.list.iter().all(|e| e.range == StringRange::between(5, 9)));
    }

    #[test]
    fn the_body_is_consumed_exactly() {
        // A tooltip's optional boolean is one byte and its component is an NBT
        // tag; mis-reading either leaves the reader inside the next entry.
        let b = reply_bytes(1, 2, 3, &[("a", Some("t")), ("b", None), ("c", Some("u"))]);
        let mut r = PacketReader::new(&b);
        for _ in 0..3 {
            r.varint().unwrap();
        }
        let n = r.count("entries", 2).unwrap();
        for _ in 0..n {
            r.string(32767).unwrap();
            r.option(|r| Ok(r.nbt()?.to_plain_text())).unwrap();
        }
        assert_eq!(r.remaining(), 0);
    }

    // ── the completion list ──────────────────────────────────────────────

    #[test]
    fn set_replaces_the_list_rather_than_adding_to_it() {
        let mut s = SuggestionProviderState::new();
        s.apply_completions(CompletionAction::Add, &["one".into(), "two".into()]);
        s.apply_completions(CompletionAction::Set, &["three".into()]);
        assert_eq!(s.custom_completions().collect::<Vec<_>>(), ["three"]);
    }

    #[test]
    fn remove_takes_only_the_named_entries_and_ignores_the_rest() {
        let mut s = SuggestionProviderState::new();
        s.apply_completions(CompletionAction::Add, &["a".into(), "b".into()]);
        s.apply_completions(CompletionAction::Remove, &["b".into(), "absent".into()]);
        assert_eq!(s.custom_completions().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn an_out_of_range_action_is_an_error_not_an_add() {
        // `readEnum` indexes an array, so vanilla throws. Treating it as ADD
        // would poison the set with words the server asked to remove.
        let mut b = Vec::new();
        varint(3, &mut b);
        varint(0, &mut b);
        assert!(read_custom_chat_completions(&b).is_err());
        let mut ok = Vec::new();
        varint(2, &mut ok);
        varint(0, &mut ok);
        assert_eq!(
            read_custom_chat_completions(&ok).unwrap(),
            (CompletionAction::Set, vec![])
        );
    }

    #[test]
    fn tab_suggestions_are_the_union_and_a_name_in_both_appears_once() {
        let mut s = SuggestionProviderState::new();
        s.apply_completions(CompletionAction::Add, &["Steve".into(), "!warp".into()]);
        assert_eq!(
            s.tab_suggestions(["Steve", "Alex"]),
            ["!warp", "Alex", "Steve"]
        );
    }

    #[test]
    fn with_no_custom_words_the_suggestions_are_the_players() {
        let s = SuggestionProviderState::new();
        assert_eq!(s.tab_suggestions(["Steve", "Alex"]), ["Alex", "Steve"]);
    }

    // ── the pending slot ─────────────────────────────────────────────────

    #[test]
    fn the_first_request_on_a_connection_is_id_zero() {
        // `++this.pendingSuggestionsId` from -1.
        let mut s = SuggestionProviderState::new();
        let (id, _) = s.begin_request("/g");
        assert_eq!(id, 0);
        assert_eq!(s.pending_id(), Some(0));
    }

    #[test]
    fn a_reply_to_a_superseded_request_is_dropped() {
        // The behaviour that stops a slow server repainting the popup with the
        // answer to a prefix you have already typed past.
        let mut s = SuggestionProviderState::new();
        s.begin_request("/g");
        s.begin_request("/gi");
        let stale = CommandSuggestionsReply {
            id: 0,
            start: 0,
            length: 0,
            entries: vec![("stale".into(), None)],
        };
        assert_eq!(s.complete(&stale), None);
        let fresh = CommandSuggestionsReply { id: 1, ..stale };
        assert!(s.complete(&fresh).is_some());
    }

    #[test]
    fn a_repeated_reply_is_dropped_because_the_slot_was_cleared() {
        let mut s = SuggestionProviderState::new();
        s.begin_request("/g");
        let reply = CommandSuggestionsReply {
            id: 0,
            start: 0,
            length: 0,
            entries: vec![("give".into(), None)],
        };
        assert!(s.complete(&reply).is_some());
        assert!(s.complete(&reply).is_none());
    }

    #[test]
    fn a_reply_arriving_while_nothing_is_outstanding_is_inert_even_at_minus_one() {
        // Vanilla's idle sentinel IS -1, and the id is a signed VarInt, so a
        // server sending -1 here satisfies its `id == pendingSuggestionsId`
        // and then dereferences a null future. Modelling the slot as an
        // Option makes the sentinel unrepresentable.
        let mut s = SuggestionProviderState::new();
        for id in [-1, 0, 7] {
            let reply = CommandSuggestionsReply {
                id,
                start: 0,
                length: 0,
                entries: vec![],
            };
            assert_eq!(s.complete(&reply), None, "id {id}");
        }
    }

    // ── the serverbound body ─────────────────────────────────────────────

    #[test]
    fn the_request_body_is_the_id_then_the_command() {
        let body = write_command_suggestion(4, "/give @s ");
        let mut r = PacketReader::new(&body);
        assert_eq!(r.varint().unwrap(), 4);
        assert_eq!(r.string(32767).unwrap(), "/give @s ");
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_command_longer_than_the_cap_is_truncated_to_it() {
        // 32500, which is neither `readUtf`'s default nor the chat screen's
        // 256 — and a server rejects a longer one.
        let long = "a".repeat(COMMAND_SUGGESTION_MAX_LEN + 10);
        let body = write_command_suggestion(0, &long);
        let mut r = PacketReader::new(&body);
        r.varint().unwrap();
        assert_eq!(r.string(32767).unwrap().len(), COMMAND_SUGGESTION_MAX_LEN);
    }
}
