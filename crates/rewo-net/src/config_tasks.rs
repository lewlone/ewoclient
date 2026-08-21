//! The two **blocking** configuration tasks (M166): the server's code of
//! conduct and its resource pack.
//!
//! **This is a hang, not a missing decode.** `REWO_PLAN.md` §0.0 named half of
//! it — `resource-pack=` — and the other half sat in the comment on the very
//! arm that swallowed both. Before M166, a server setting either of these
//! meant `rewo live` **never opened a window and never errored**: it sat in
//! `run_configuration` forever, answering keep-alives.
//!
//! ## Why it hangs rather than failing
//!
//! `ServerConfigurationPacketListenerImpl` runs its tasks **strictly one at a
//! time**: `startNextTask` polls one off the queue, `start()`s it, and refuses
//! to advance until something calls `finishCurrentTask` (`:206-232`). Most
//! tasks finish themselves — `SynchronizeRegistriesTask` on the client's
//! `select_known_packs` reply, `JoinWorldTask` on `finish_configuration`. The
//! two that `addOptionalTasks` (`:110-128`) appends do **not**:
//!
//! | task | `start()` sends | finished by |
//! |---|---|---|
//! | `ServerCodeOfConductConfigurationTask` | `code_of_conduct` (cfg cb **19**) | `accept_code_of_conduct` (cfg sb **9**) |
//! | `ServerResourcePackConfigurationTask` | `resource_pack_push` (cfg cb **9**) | `resource_pack` (cfg sb **6**) with a **terminal** action |
//!
//! So the queue stalls, `finish_configuration` is never reached, and the
//! client is left in a state that looks identical to a healthy one from the
//! outside: **the socket is live and the 15 s keep-alives keep arriving**, so
//! Rewo's 30 s read timeout can never fire either. Silence is the whole
//! symptom.
//!
//! The code of conduct is appended **first**, so on a server with both it is
//! the one that hangs and the resource pack is never even sent.
//!
//! ## The class-C label was wrong, for the sixth time
//!
//! `REWO_PACKET_COVERAGE.md` filed rows 80/81 as class **C** — "server
//! resource-pack pipeline (download, prompt, apply)" — naming a subsystem
//! Rewo genuinely lacks. But nothing about *unblocking the task* needs that
//! pipeline. It needs a 17-byte reply. That is the same misreading as M91's
//! furnace recipes, M93's merchant quick-move, M93s's stonecutter list,
//! M93u's merchant offers and M152's smithing sets: **a blocker stated from
//! the wire's point of view can be wrong because the answer was never on the
//! wire.**
//!
//! ## `FAILED_DOWNLOAD`, and the guard that makes it available
//!
//! Which terminal action to send is a real choice, because
//! `ServerCommonPacketListenerImpl:107` reads:
//!
//! ```text
//! if (packet.action() == Action.DECLINED && this.server.isResourcePackRequired())
//! ```
//!
//! — **only `DECLINED`**, and against the server's *own* `require-resource-pack`
//! setting rather than the `required` flag it just sent. So `DECLINED` is the
//! one terminal action that gets you kicked from a required-pack server (which
//! is correct vanilla behaviour: `PackConfirmScreen`'s decline arm disconnects
//! the client itself). `FAILED_DOWNLOAD` is equally terminal, is *true* — Rewo
//! did not fetch it — and slips past that guard, so a required-pack server
//! stays joinable without Rewo ever claiming to have applied a pack it will
//! not render. That is the shipped choice, made by the user.
//!
//! ## Four rules where the plausible implementation is silently wrong
//!
//! 1. **`isTerminal()` is a denial-list, not an allow-list** —
//!    `action != ACCEPTED && action != DOWNLOADED`. Six of the eight actions
//!    finish the task. Reading it as "SUCCESSFULLY_LOADED finishes it" and
//!    sending anything else leaves the client hung exactly as before.
//! 2. **`writeEnum` writes the ordinal**, so the wire value is the enum's
//!    **declaration order** — `SUCCESSFULLY_LOADED` 0, `DECLINED` 1,
//!    `FAILED_DOWNLOAD` 2 — which is *not* the order an accept/decline/fail
//!    mental model would give. An off-by-one here sends `DECLINED` while
//!    meaning `FAILED_DOWNLOAD`, and gets you kicked.
//! 3. **The reply is not one packet per push in vanilla.**
//!    `DownloadedPackSource` reports progress (`ACCEPTED`, `DOWNLOADED` — both
//!    non-terminal) and then a final result. Rewo has no download, so it sends
//!    the final result alone; the intermediate ones are optional by
//!    construction, since the server finishes on the terminal one and does
//!    nothing with the rest.
//! 4. **`accept_code_of_conduct` has a zero-byte body** (`StreamCodec.unit`).
//!    A reply carrying anything at all desynchronises the server's reader.
//!
//! ## What is deliberately NOT here
//!
//! * **`resource_pack_pop`** (cfg cb 8 / play cb 80) stays **absent** rather
//!   than resolved-and-ignored. It needs no reply, blocks no task, and Rewo has
//!   no pack to remove, so decoding it would create a model with no consumer —
//!   M151's trap — and *resolving* it would put a row in the coverage doc's
//!   `ignored` class, which M74 argues is worse than `absent` because it reads
//!   as handled to every grep.
//! * **The prompt component** is decoded (it must be, to walk the body) and
//!   not displayed: Rewo has no pack-confirm screen, and prompting for a
//!   choice the client has already made would be a lie in the other direction.

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::writer::PacketWriter;
use rewo_proto::Result as ProtoResult;

/// `ServerboundResourcePackPacket.Action`, in **declaration order** — the
/// ordinal is the wire value (`FriendlyByteBuf.writeEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackAction {
    SuccessfullyLoaded = 0,
    Declined = 1,
    FailedDownload = 2,
    Accepted = 3,
    Downloaded = 4,
    InvalidUrl = 5,
    FailedReload = 6,
    Discarded = 7,
}

impl PackAction {
    /// The VarInt written on the wire.
    pub fn ordinal(self) -> i32 {
        self as i32
    }

    /// Vanilla's `isTerminal()`: **everything except the two progress
    /// reports**. This is what `finishCurrentTask` gates on, so a reply whose
    /// action is not terminal leaves the configuration queue stalled exactly
    /// as sending nothing would.
    pub fn is_terminal(self) -> bool {
        !matches!(self, PackAction::Accepted | PackAction::Downloaded)
    }
}

/// One decoded `resource_pack_push`.
#[derive(Debug, Clone, PartialEq)]
pub struct PackPush {
    pub id: u128,
    pub url: String,
    pub hash: String,
    pub required: bool,
    pub prompt: Option<Nbt>,
}

/// `ClientboundResourcePackPushPacket.STREAM_CODEC` — UUID, url, hash
/// (`stringUtf8(40)`), bool, optional component.
///
/// The whole body is walked even though only the first field is replied with.
/// A decode that stopped after the UUID could not tell a well-formed packet
/// from a malformed one, and this is a packet whose mis-decode is otherwise
/// invisible: the reply would still be sent, just addressed to a UUID the
/// server never issued, and `finishCurrentTask` does not check the id.
pub fn read_pack_push(body: &[u8]) -> ProtoResult<PackPush> {
    let mut r = PacketReader::new(body);
    let id = r.uuid()?;
    let url = r.string(32767)?;
    let hash = r.string(40)?;
    let required = r.bool()?;
    let prompt = if r.bool()? { Some(r.nbt()?) } else { None };
    Ok(PackPush {
        id,
        url,
        hash,
        required,
        prompt,
    })
}

/// Vanilla's `parseResourcePackUrl`, reduced to the part that is observable.
///
/// The Java is `new URL(s)` followed by a protocol test against `"http"` /
/// `"https"`, and **every** way of failing lands on the same `null`: an
/// unknown scheme throws `MalformedURLException`, a *known* non-web scheme
/// (`ftp:`, `file:`) parses and then fails the protocol test, and a string
/// with no scheme throws. Three failure routes, one answer — so the rule is
/// just "the scheme is http or https".
///
/// `URL(String)` lowercases the scheme before its handler lookup, so the
/// comparison is ASCII-case-insensitive.
///
/// **Stated deviation:** `java.net.URL` also rejects a handful of strings that
/// *do* start `http:` — an unterminated IPv6 literal, say — where this returns
/// `true` and Rewo answers `FAILED_DOWNLOAD` instead of `INVALID_URL`. Both
/// are terminal, neither triggers the required-pack kick, and Rewo has no URL
/// parser to reproduce the rest with. The difference is one word in a server
/// log.
pub fn pack_url_loadable(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
}

/// The action Rewo replies with, given the pushed URL.
///
/// Both arms are terminal, so either finishes the task; the split exists
/// because vanilla makes it, and because it is the one branch of
/// `handleResourcePackPush` that runs without a pack pipeline underneath it.
pub fn reply_action(url: &str) -> PackAction {
    if pack_url_loadable(url) {
        // Truthful: Rewo has no `DownloadedPackSource`, so the download did
        // not happen. Chosen over `DECLINED` because only `DECLINED` is kicked
        // on a `require-resource-pack=true` server — see the module doc.
        PackAction::FailedDownload
    } else {
        PackAction::InvalidUrl
    }
}

/// `ServerboundResourcePackPacket` body: UUID + the action's ordinal as a
/// VarInt. 17 bytes of body for every action, since all eight ordinals fit in
/// one VarInt byte.
pub fn write_pack_reply(packet_id: i32, id: u128, action: PackAction) -> PacketWriter {
    let mut p = PacketWriter::packet(packet_id);
    p.uuid(id);
    p.varint(action.ordinal());
    p
}

/// What the two blocking configuration tasks asked for, and what Rewo
/// answered.
///
/// Held so the reply is provably a response to something *decoded* rather than
/// to the arrival of a packet id. A counter would be satisfied by any
/// implementation that reached the arm — including one that replied with a
/// zero UUID — and the UUID is the only field of the push that leaves the
/// client again.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ConfigTaskLog {
    /// One entry per `resource_pack_push` answered, in arrival order, across
    /// **both** states: a mid-session push lands here beside a join-time one.
    pub pack_replies: Vec<(u128, PackAction)>,
    /// The code-of-conduct texts accepted, in arrival order. Configuration
    /// only.
    pub codes_of_conduct: Vec<String>,
}

/// Decode one `resource_pack_push`, record it, and decide the reply.
///
/// Shared by both states because vanilla's `handleResourcePackPush` is on
/// `ClientCommonPacketListenerImpl` — one handler, two ids.
///
/// **A body that fails to decode is still answered**, with a zero id. The
/// alternative is silence, and silence is the bug this module exists to fix:
/// on the configuration path it hangs the connection outright. A reply naming
/// a UUID the server never issued is harmless by comparison, because
/// `finishCurrentTask` matches on the task *type* and never looks at the id.
pub fn answer_pack_push(body: &[u8], log: &mut ConfigTaskLog) -> (u128, PackAction) {
    let (id, action) = match read_pack_push(body) {
        Ok(push) => {
            let action = reply_action(&push.url);
            log::info!(
                "net: resource pack {:032x} ({}, required={}) -> {:?}",
                push.id,
                push.url,
                push.required,
                action
            );
            (push.id, action)
        }
        Err(err) => {
            log::warn!("net: resource_pack_push decode: {err} — answering with a zero id");
            (0, PackAction::FailedDownload)
        }
    };
    debug_assert!(
        action.is_terminal(),
        "a non-terminal reply leaves the server's configuration queue stalled"
    );
    log.pack_replies.push((id, action));
    (id, action)
}

/// `ClientboundCodeOfConductPacket` — a single string.
///
/// The text is returned rather than dropped so that the reply is provably a
/// response to something decoded, and so `rewo live` can log what it accepted
/// on the user's behalf.
pub fn read_code_of_conduct(body: &[u8]) -> ProtoResult<String> {
    PacketReader::new(body).string(32767)
}

/// `ServerboundAcceptCodeOfConductPacket` — `StreamCodec.unit`, i.e. the
/// packet id and **nothing else**.
pub fn write_code_of_conduct_accept(packet_id: i32) -> PacketWriter {
    PacketWriter::packet(packet_id)
}

#[cfg(test)]
mod tests {
    //! Witnesses for [`crate::config_tasks`] (M166).
    //!
    //! Every expected value here is a **literal read out of the decompile**, not a
    //! value derived from the module under test. The ordinals in particular are
    //! written out longhand in `ServerboundResourcePackPacket.Action`'s declaration
    //! order rather than computed from `PackAction`, because computing them would
    //! assert only that the enum equals itself — which is the exact shape M93r's
    //! sweep was run to find.
    use super::*;

    use rewo_proto::writer::PacketWriter;

    /// A `resource_pack_push` body, built the way the server's `STREAM_CODEC`
    /// composes it. This is the *only* place these bytes are laid out; the module
    /// under test never sees the writer.
    fn push_body(id: u128, url: &str, hash: &str, required: bool, prompt: Option<&str>) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.uuid(id);
        w.string(url);
        w.string(hash);
        w.bool(required);
        match prompt {
            // A component is one NBT tag (M41's `fromCodecWithRegistries` rule);
            // the simplest well-formed one is a bare TAG_String.
            Some(text) => {
                w.bool(true);
                w.u8(8); // TAG_String
                w.u16(text.len() as u16);
                w.raw(text.as_bytes());
            }
            None => {
                w.bool(false);
            }
        }
        w.into_bytes()
    }

    #[test]
    fn action_ordinals_are_the_java_declaration_order() {
        // net/minecraft/network/protocol/common/ServerboundResourcePackPacket.java,
        // enum Action, read top to bottom. `FriendlyByteBuf.writeEnum` writes
        // `ordinal()`, so these literals ARE the wire values.
        assert_eq!(PackAction::SuccessfullyLoaded.ordinal(), 0);
        assert_eq!(PackAction::Declined.ordinal(), 1);
        assert_eq!(PackAction::FailedDownload.ordinal(), 2);
        assert_eq!(PackAction::Accepted.ordinal(), 3);
        assert_eq!(PackAction::Downloaded.ordinal(), 4);
        assert_eq!(PackAction::InvalidUrl.ordinal(), 5);
        assert_eq!(PackAction::FailedReload.ordinal(), 6);
        assert_eq!(PackAction::Discarded.ordinal(), 7);
    }

    #[test]
    fn is_terminal_is_a_denial_list_of_exactly_two() {
        // `return this != ACCEPTED && this != DOWNLOADED;` — so six of the eight
        // finish the server's task. Stated positively here so that widening OR
        // narrowing the set fails.
        let terminal = [
            PackAction::SuccessfullyLoaded,
            PackAction::Declined,
            PackAction::FailedDownload,
            PackAction::InvalidUrl,
            PackAction::FailedReload,
            PackAction::Discarded,
        ];
        for a in terminal {
            assert!(a.is_terminal(), "{a:?} finishes the task in vanilla");
        }
        for a in [PackAction::Accepted, PackAction::Downloaded] {
            assert!(!a.is_terminal(), "{a:?} is a progress report, not a result");
        }
    }

    #[test]
    fn both_shipped_replies_are_terminal() {
        // The claim that matters for the hang: whichever arm `reply_action` takes,
        // the server's queue advances. A milestone that changed either constant to
        // ACCEPTED would reintroduce the exact bug M166 fixes, and would look
        // perfectly reasonable in a diff.
        assert!(reply_action("https://example.com/p.zip").is_terminal());
        assert!(reply_action("ftp://example.com/p.zip").is_terminal());
    }

    #[test]
    fn declined_is_the_one_action_that_gets_you_kicked() {
        // ServerCommonPacketListenerImpl:107 — `action() == DECLINED &&
        // isResourcePackRequired()`. This pins the reason FAILED_DOWNLOAD was
        // chosen: if a later change makes `reply_action` return DECLINED, a
        // required-pack server disconnects the client and the fix silently
        // regresses into a different failure.
        assert_ne!(reply_action("https://example.com/p.zip"), PackAction::Declined);
        assert_ne!(reply_action("nonsense"), PackAction::Declined);
    }

    #[test]
    fn pack_url_follows_the_protocol_test_and_not_the_parse() {
        // `parseResourcePackUrl`: new URL(s), then protocol must be http/https.
        // URL(String) lowercases the scheme before its handler lookup, so the
        // comparison is case-insensitive.
        for ok in [
            "http://example.com/pack.zip",
            "https://example.com/pack.zip",
            "HTTP://EXAMPLE.COM/p.zip",
            "HtTpS://example.com/p.zip",
            // No host, no path — `new URL("http:")` throws nothing.
            "http:",
        ] {
            assert!(pack_url_loadable(ok), "{ok} is loadable in vanilla");
        }
        for bad in [
            // Parses as a URL (a handler exists) and then fails the protocol test.
            "ftp://example.com/p.zip",
            "file:///C:/p.zip",
            // Throws MalformedURLException — same answer by a different route.
            "gopher://example.com/p.zip",
            // No scheme at all.
            "example.com/pack.zip",
            "",
        ] {
            assert!(!pack_url_loadable(bad), "{bad} yields null in vanilla");
        }
    }

    #[test]
    fn https_is_not_matched_by_a_prefix_test() {
        // A `starts_with("http")` implementation accepts these and is wrong for
        // every one. The split is on the SCHEME, i.e. the text before the colon.
        for bad in ["httpx://example.com/p.zip", "httpfoo:", "http-ish://x"] {
            assert!(!pack_url_loadable(bad), "{bad} has scheme != http/https");
        }
    }

    #[test]
    fn reply_action_splits_on_url_validity() {
        assert_eq!(
            reply_action("https://example.com/pack.zip"),
            PackAction::FailedDownload
        );
        assert_eq!(reply_action("ftp://example.com/pack.zip"), PackAction::InvalidUrl);
    }

    #[test]
    fn push_decodes_every_field() {
        let id = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210u128;
        let body = push_body(id, "https://example.com/p.zip", "abc123", true, None);
        let push = read_pack_push(&body).expect("well-formed push");
        assert_eq!(push.id, id);
        assert_eq!(push.url, "https://example.com/p.zip");
        assert_eq!(push.hash, "abc123");
        assert!(push.required);
        assert!(push.prompt.is_none());
    }

    #[test]
    fn the_optional_prompt_is_walked_rather_than_assumed_absent() {
        // The prompt is the LAST field, so a reader that ignored it would still
        // return the right first four. What this pins is that the flag is read at
        // all — a reader that stopped after `required` reports `prompt: None` for
        // a push that carries one, and would pass every other test in this file.
        let id = 7;
        let body = push_body(id, "https://x/p.zip", "", false, Some("Please accept"));
        let push = read_pack_push(&body).expect("well-formed push with a prompt");
        assert!(push.prompt.is_some(), "the prompt flag was not read");
        assert!(!push.required);
    }

    #[test]
    fn a_truncated_push_is_an_error_rather_than_a_default() {
        // Half a UUID. The point is that this does not silently yield id 0 with a
        // plausible-looking empty url.
        assert!(read_pack_push(&[0u8; 8]).is_err());
        assert!(read_pack_push(&[]).is_err());
    }

    #[test]
    fn the_reply_body_is_seventeen_bytes_after_the_packet_id() {
        let id = 0xdead_beef_dead_beef_dead_beef_dead_beefu128;
        let bytes = write_pack_reply(6, id, PackAction::FailedDownload).into_bytes();
        // packet id (1 VarInt byte for 6) + 16 UUID + 1 action = 18.
        assert_eq!(bytes.len(), 18, "{bytes:02x?}");
        assert_eq!(bytes[0], 6, "packet id");
        assert_eq!(&bytes[1..17], &id.to_be_bytes(), "UUID is big-endian hi..lo");
        assert_eq!(bytes[17], 2, "FAILED_DOWNLOAD ordinal");
    }

    #[test]
    fn the_code_of_conduct_accept_carries_no_body() {
        // `StreamCodec.unit` — the packet id and nothing else. A reply with a
        // payload desynchronises the server's reader for every packet after it.
        let bytes = write_code_of_conduct_accept(9).into_bytes();
        assert_eq!(bytes, vec![9u8]);
    }

    #[test]
    fn code_of_conduct_decodes_its_string() {
        let mut w = PacketWriter::default();
        w.string("Be excellent to each other.");
        let text = read_code_of_conduct(&w.into_bytes()).expect("well-formed");
        assert_eq!(text, "Be excellent to each other.");
    }

    #[test]
    fn answer_records_the_decoded_id_and_not_a_placeholder() {
        let id = 0x1111_2222_3333_4444_5555_6666_7777_8888u128;
        let body = push_body(id, "https://example.com/p.zip", "", false, None);
        let mut log = ConfigTaskLog::default();
        let (replied, action) = answer_pack_push(&body, &mut log);
        assert_eq!(replied, id);
        assert_eq!(action, PackAction::FailedDownload);
        assert_eq!(log.pack_replies, vec![(id, PackAction::FailedDownload)]);
    }

    #[test]
    fn a_malformed_push_is_still_answered_terminally() {
        // Silence is the bug. A body that cannot be decoded must not turn into a
        // hung connection, so the reply is sent with a zero id — which the server
        // accepts, because `finishCurrentTask` matches on the task TYPE and never
        // reads the id back.
        let mut log = ConfigTaskLog::default();
        let (replied, action) = answer_pack_push(&[0u8; 3], &mut log);
        assert_eq!(replied, 0);
        assert!(action.is_terminal());
        assert_eq!(log.pack_replies.len(), 1, "the failure is recorded, not dropped");
    }

    #[test]
    fn several_pushes_accumulate_in_arrival_order() {
        // A per-world pack swap sends one push per world change, and the play-state
        // arm shares this log with the configuration one.
        let mut log = ConfigTaskLog::default();
        for id in 1..=3u128 {
            let body = push_body(id, "https://example.com/p.zip", "", false, None);
            answer_pack_push(&body, &mut log);
        }
        assert_eq!(
            log.pack_replies.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}
