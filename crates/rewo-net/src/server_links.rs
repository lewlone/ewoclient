//! `ClientboundServerLinksPacket` — the links a server advertises (M85).
//!
//! A small packet with a large consequence: it is the last class-**B** entry in
//! `REWO_PACKET_COVERAGE.md` alongside `award_stats`, and what made it class B
//! was never the decode — it was that vanilla renders it on two screens Rewo
//! did not have.
//!
//! # Where vanilla actually puts them — three corrections
//!
//! The brief for this milestone said "vanilla draws server links on the pause
//! screen and the disconnect screen". Both halves are off, and the shapes are
//! worth recording because each one changes what has to be built.
//!
//! 1. **The pause screen shows one button, not a list.**
//!    `PauseScreen.getCustomAdditions()` ends with
//!
//!    ```java
//!    ServerLinks serverLinks = this.minecraft.player.connection.serverLinks();
//!    return !serverLinks.isEmpty() ? dialogRegistry.get(Dialogs.SERVER_LINKS) : Optional.empty();
//!    ```
//!
//!    and `addCustomDialogButtons` turns that into a **single** 204-wide button
//!    labelled `dialog.value().common().computeExternalTitle()`. The list lives
//!    on a separate screen — a `ServerLinksDialogScreen`, one button per entry —
//!    which the button opens. See [`crate::server_links`]'s consumers in
//!    `rewo_world::{pause_screen, server_links_screen}`.
//!
//! 2. **The disconnect screen shows at most *one* link, and only ever
//!    `BUG_REPORT`.** `DisconnectedScreen` never mentions `ServerLinks` at all.
//!    What reaches it is `DisconnectionDetails.bugReportLink`, an
//!    `Optional<URI>` filled by
//!    `serverLinks.findKnownType(KnownLinkType.BUG_REPORT).map(Entry::link)` in
//!    `onPacketError` and `createDisconnectionInfo` — the two *client-side
//!    error* paths. A clean `ClientboundDisconnectPacket` goes through
//!    `connection.disconnect(reason)`, which builds `new
//!    DisconnectionDetails(reason)` with **both** optionals empty, so a server
//!    that kicks you politely shows no link however many it advertised.
//!
//! 3. **The packet exists in *both* the configuration and the play state**
//!    (`ConfigurationProtocols` and `GameProtocols` each `.addPacket(
//!    CommonPacketTypes.CLIENTBOUND_SERVER_LINKS, …)`), because
//!    `handleServerLinks` is on `ClientCommonPacketListener`. M69 records the
//!    same shape for `update_tags` and M78 for `custom_payload`; the rule is
//!    now three for three. Resolving only the play id would look like it worked
//!    against any server that sends its links during configuration.
//!
//! # The wire, and what inverts
//!
//! ```text
//! ClientboundServerLinksPacket
//!   = ServerLinks.UNTRUSTED_LINKS_STREAM_CODEC
//!   = UntrustedEntry.STREAM_CODEC.apply(ByteBufCodecs.list())
//!
//! UntrustedEntry = composite(
//!     ServerLinks.TYPE_STREAM_CODEC, type,        // Either<KnownLinkType, Component>
//!     ByteBufCodecs.STRING_UTF8,     link)        // a plain String, max 32767
//! ```
//!
//! * **`ByteBufCodecs.either` writes `true` for the *left*** —
//!   `input.readBoolean() ? Either.left(…) : Either.right(…)`. M83 records the
//!   same for `FriendlyByteBuf.writeEither`, and it is the reading that a
//!   "true means the interesting case" instinct gets backwards: the *left* here
//!   is the boring enum and the *right* is the custom `Component`.
//! * **The enum is `ByIdMap.continuous(…, OutOfBoundsStrategy.ZERO)`**, read
//!   through `ByteBufCodecs.idMapper` — a **VarInt**. So an id of `10`, or a
//!   *negative* id, is not an error: it silently becomes `BUG_REPORT`, which is
//!   the one type the disconnect screen singles out. The third of M65's three
//!   conventions (`readEnum` throws, `ZERO` substitutes index 0, `WRAP` is
//!   `floorMod`) and the one that is hardest to notice, because a wrong link is
//!   still a link.
//! * **The custom label is `ComponentSerialization.TRUSTED_CONTEXT_FREE_
//!   STREAM_CODEC`** — `fromCodecTrusted`, i.e. one NBT tag with no registry
//!   access, exactly as M41 records for every other component on the wire.
//! * **The URL is a bare `String`, not a `Component` and not an `Identifier`.**
//!   That is what makes the entry *untrusted*: the client parses it itself.
//!
//! # The trust step is a filter, not a validation
//!
//! `handleServerLinks` runs each entry through
//! `Util.parseAndValidateUntrustedUri`, which requires a scheme and requires
//! that scheme — lowercased — to be in `ALLOWED_UNTRUSTED_LINK_PROTOCOLS =
//! Set.of("http", "https")`. A failure is caught **per entry**:
//!
//! ```java
//! } catch (Exception e) {
//!    LOGGER.warn("Received invalid link for type {}:{}", …);
//! }
//! ```
//!
//! so one bad link drops itself and the rest of the packet still applies. A
//! reader that rejected the whole packet would give a server one malformed
//! entry away from having no links at all, and a reader that kept the bad one
//! would put a `file:` or `javascript:` URL in front of the user. Both are
//! plausible and neither is vanilla.
//!
//! # Rewo DOES open a URL now (corrected 2026-08-20)
//!
//! **This section was headed "Rewo never opens a URL" and ended "Rewo opens
//! nothing" long after M128 shipped `uri_open.rs`** — chat links open, from
//! `live_cmd.rs`. What is still true is narrower and is the interesting part:
//! the SERVER-LINKS dialog's own buttons only log, so **one action has two call
//! sites that disagree**. The paragraphs below are kept for the warning-line
//! transcription they carry; read their "opens nothing" as "this screen's
//! buttons open nothing".
//!
//! For the record, because it is the question this milestone exists to answer:
//! vanilla's click path is `ServerLinksDialogScreen.createDialogClickAction` →
//! `StaticAction(new ClickEvent.OpenUrl(entry.link()))` →
//! `Screen.defaultHandleClickEvent` → `Screen.clickUrlAction`, which is
//!
//! ```java
//! if (!minecraft.options.chatLinks().get()) return false;          // silently nothing
//! if (minecraft.options.chatLinksPrompt().get()) {                  // the default
//!    minecraft.gui.setScreen(new ConfirmLinkScreen(result -> { … Util.getPlatform().openUri(uri); … }, uri.toString(), false));
//! } else {
//!    Util.getPlatform().openUri(uri);
//! }
//! ```
//!
//! — so even vanilla does not open a link without a confirmation screen unless
//! the player turned the prompt off, and the `trusted = false` argument adds a
//! red `chat.link.warning` line to it. **This screen's buttons open nothing**
//! (M128 gave chat links `uri_open`; these were not wired). Launching a
//! browser from a string a remote server chose is a decision, not a
//! transcription; the links render, they are selectable, and pressing one logs
//! the URL. Whether Rewo ever spawns a browser is left to the project.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/network/protocol/common/ClientboundServerLinksPacket.java`
//! - `net/minecraft/server/ServerLinks.java`
//! - `net/minecraft/network/codec/ByteBufCodecs.java` — `either`, `idMapper`,
//!   `list`, `stringUtf8`
//! - `net/minecraft/util/ByIdMap.java` — `continuous`, `OutOfBoundsStrategy`
//! - `net/minecraft/util/Util.java` — `parseAndValidateUntrustedUri`,
//!   `ALLOWED_UNTRUSTED_LINK_PROTOCOLS`
//! - `net/minecraft/client/multiplayer/ClientCommonPacketListenerImpl.java` —
//!   `handleServerLinks`, `createDisconnectionInfo`, `onPacketError`
//! - `net/minecraft/client/gui/screens/PauseScreen.java` — `getCustomAdditions`
//! - `net/minecraft/client/gui/screens/DisconnectedScreen.java`
//! - `net/minecraft/client/gui/screens/dialog/ServerLinksDialogScreen.java`

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `ServerLinks.KnownLinkType` — the ten ids a server may use instead of
/// sending its own label.
///
/// Declaration order **is** id order here (`BUG_REPORT(0, "report_bug")` …
/// `ANNOUNCEMENTS(9, "announcements")`), but the ids are written explicitly in
/// the enum rather than taken from `ordinal()`, so [`Self::VALUES`] is ordered
/// by the declared id and not by position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnownLinkType {
    BugReport,
    CommunityGuidelines,
    Support,
    Status,
    Feedback,
    Community,
    Website,
    Forums,
    News,
    Announcements,
}

impl KnownLinkType {
    /// Sorted by the declared id, which is what `ByIdMap.createSortedArray`
    /// produces.
    pub const VALUES: [KnownLinkType; 10] = [
        KnownLinkType::BugReport,
        KnownLinkType::CommunityGuidelines,
        KnownLinkType::Support,
        KnownLinkType::Status,
        KnownLinkType::Feedback,
        KnownLinkType::Community,
        KnownLinkType::Website,
        KnownLinkType::Forums,
        KnownLinkType::News,
        KnownLinkType::Announcements,
    ];

    /// `ByIdMap.continuous(e -> e.id, values(), ZERO)`:
    ///
    /// ```java
    /// case ZERO -> { T zeroValue = sortedValues[0];
    ///                yield id -> id >= 0 && id < length ? sortedValues[id] : zeroValue; }
    /// ```
    ///
    /// **Total, and the substitute is `BUG_REPORT`.** Not an error, not a
    /// clamp, not a wrap — and `BUG_REPORT` is precisely the type the
    /// disconnect screen looks for, so an out-of-range id from a newer server
    /// does not merely mislabel a button, it can put a link on the disconnect
    /// screen that was never meant to be one.
    pub fn by_id(id: i32) -> KnownLinkType {
        if id >= 0 && (id as usize) < KnownLinkType::VALUES.len() {
            KnownLinkType::VALUES[id as usize]
        } else {
            KnownLinkType::VALUES[0]
        }
    }

    /// The declared id, which is what `idMapper` writes.
    pub fn id(self) -> i32 {
        KnownLinkType::VALUES
            .iter()
            .position(|&v| v == self)
            .expect("VALUES is exhaustive") as i32
    }

    /// The `name` half of the enum constant.
    ///
    /// **`BUG_REPORT`'s name is `report_bug`, not `bug_report`** — the constant
    /// and its string are transposed, and nothing else in the table is. A
    /// lang key derived from the constant name would come out
    /// `known_server_link.bug_report`, which does not exist, and the button
    /// would render its own key.
    pub fn name(self) -> &'static str {
        match self {
            KnownLinkType::BugReport => "report_bug",
            KnownLinkType::CommunityGuidelines => "community_guidelines",
            KnownLinkType::Support => "support",
            KnownLinkType::Status => "status",
            KnownLinkType::Feedback => "feedback",
            KnownLinkType::Community => "community",
            KnownLinkType::Website => "website",
            KnownLinkType::Forums => "forums",
            KnownLinkType::News => "news",
            KnownLinkType::Announcements => "announcements",
        }
    }

    /// `displayName()` — `Component.translatable("known_server_link." + name)`.
    pub fn lang_key(self) -> String {
        format!("known_server_link.{}", self.name())
    }
}

/// `Either<KnownLinkType, Component>` — a link's label.
///
/// The variant names follow the `Either`: `Known` is the **left** and is what
/// the boolean `true` selects.
/// Not `Eq`: a custom label is a component now (M129), and `Nbt` carries floats.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerLinkLabel {
    /// `Either.left` — wire boolean `true`, then a VarInt id.
    Known(KnownLinkType),
    /// `Either.right` — wire boolean `false`, then one NBT tag.
    ///
    /// **Unflattened (M129), and the asymmetry is why.** The screen already
    /// renders `Known` through the language table (`lang.or_key(t.lang_key())`)
    /// and rendered `Custom` as a raw flattened string, so one of the two label
    /// kinds honoured the player's language and the other did not. Resolution
    /// happens at the render site rather than here because `read_server_links`
    /// runs in **both** connection states, and the configuration path has no
    /// `PlaySession` and therefore no table — the same reason M125 moved the
    /// chat components rather than resolving them at the wire.
    Custom(rewo_proto::nbt::Nbt),
}

/// `ServerLinks.UntrustedEntry` — straight off the wire, URL unvalidated.
#[derive(Clone, Debug, PartialEq)]
pub struct UntrustedEntry {
    pub label: ServerLinkLabel,
    pub link: String,
}

/// `ServerLinks.Entry` — an entry whose URL passed
/// `parseAndValidateUntrustedUri`.
///
/// The URL is kept as the original string rather than a parsed type: Rewo has
/// no URI type and never dereferences it, and vanilla's own `Entry.link` is
/// only ever `toString()`ed back for display, the clipboard, and a crash-report
/// comment.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerLink {
    pub label: ServerLinkLabel,
    pub link: String,
}

/// `ServerLinks` — the trusted list. `ServerLinks.EMPTY` is [`Default`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerLinks {
    entries: Vec<ServerLink>,
}

impl ServerLinks {
    pub fn new(entries: Vec<ServerLink>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[ServerLink] {
        &self.entries
    }

    /// `ServerLinks.isEmpty` — the whole test `PauseScreen.getCustomAdditions`
    /// makes before deciding whether the pause menu grows a button.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `findKnownType(type)`:
    ///
    /// ```java
    /// this.entries.stream().filter(e -> e.type.map(l -> l == type, r -> false)).findFirst();
    /// ```
    ///
    /// **`findFirst`, and a custom label never matches.** The `r -> false` is
    /// the half worth naming: a server whose custom label happens to read
    /// "Report a bug" is not a `BUG_REPORT` entry and cannot reach the
    /// disconnect screen. And it is the *first* matching entry, so a server
    /// that sends two `BUG_REPORT`s advertises the earlier one.
    pub fn find_known_type(&self, want: KnownLinkType) -> Option<&ServerLink> {
        self.entries
            .iter()
            .find(|e| matches!(&e.label, ServerLinkLabel::Known(t) if *t == want))
    }
}

/// `Util.ALLOWED_UNTRUSTED_LINK_PROTOCOLS`.
pub const ALLOWED_UNTRUSTED_LINK_PROTOCOLS: [&str; 2] = ["http", "https"];

/// `Util.parseAndValidateUntrustedUri` — reduced to the two things it can
/// reject on.
///
/// ```java
/// URI parsedUri = new URI(uri);
/// String scheme = parsedUri.getScheme();
/// if (scheme == null) throw …;
/// if (!ALLOWED_UNTRUSTED_LINK_PROTOCOLS.contains(scheme.toLowerCase(Locale.ROOT))) throw …;
/// return parsedUri;
/// ```
///
/// Rewo has no URI parser, so `new URI(uri)`'s own syntax rejection is **not**
/// reproduced — a string that Java's RFC-2396 parser would refuse (a raw space,
/// an unbracketed IPv6 host) is accepted here. That is a stated widening, and
/// it is the safe direction: the check that matters for a link the user might
/// be shown is the scheme allowlist, and it is exact. Anything the widening
/// lets through is still an `http`/`https` string with no way to become
/// anything else, because nothing dereferences it (see the module docs).
///
/// The scheme is everything before the first `:`, and it must be non-empty —
/// `":80/x"` has a `null` scheme in Java, not an empty one.
pub fn validate_untrusted_uri(uri: &str) -> Option<&str> {
    let scheme = uri.split(':').next().filter(|s| !s.is_empty())?;
    // Java's `getScheme` is `null` unless the part before the colon is a legal
    // scheme, and a URI with no colon at all has no scheme. `split(':').next()`
    // on a colonless string yields the whole string, so the colon must be
    // checked separately or `"example.com"` would be tested as a scheme.
    if !uri[scheme.len()..].starts_with(':') {
        return None;
    }
    let lower = scheme.to_ascii_lowercase();
    ALLOWED_UNTRUSTED_LINK_PROTOCOLS
        .contains(&lower.as_str())
        .then_some(uri)
}

/// `ServerLinks.UNTRUSTED_LINKS_STREAM_CODEC` — a VarInt count then that many
/// `UntrustedEntry`s.
pub fn read_server_links(body: &[u8]) -> Result<Vec<UntrustedEntry>> {
    let mut r = PacketReader::new(body);
    // `ByteBufCodecs.list()` is the unbounded `collection`, so the count is
    // guarded by the buffer. The floor is 2 bytes: one for the `Either` flag
    // and one for a zero-length URL.
    let n = r.count("server links", 2)?;
    let mut out = Vec::with_capacity(n.min(64) as usize);
    for _ in 0..n {
        let label = if r.bool()? {
            // `Either.left` — the enum. See the module docs: the flag is
            // `true` for the *left*, which is the boring case.
            ServerLinkLabel::Known(KnownLinkType::by_id(r.varint()?))
        } else {
            ServerLinkLabel::Custom(r.nbt()?)
        };
        let link = r.string(32767)?;
        out.push(UntrustedEntry { label, link });
    }
    Ok(out)
}

/// `handleServerLinks`' body — validate each URL, **dropping** the entries that
/// fail and keeping the rest.
///
/// Returns the trusted list plus how many entries were dropped, so a caller can
/// log what vanilla logs (`LOGGER.warn("Received invalid link for type {}:{}")`)
/// without this function taking a logger.
pub fn trust(entries: Vec<UntrustedEntry>) -> (ServerLinks, usize) {
    let mut dropped = 0usize;
    let kept = entries
        .into_iter()
        .filter_map(|e| {
            if validate_untrusted_uri(&e.link).is_some() {
                Some(ServerLink {
                    label: e.label,
                    link: e.link,
                })
            } else {
                dropped += 1;
                None
            }
        })
        .collect();
    (ServerLinks::new(kept), dropped)
}

#[cfg(test)]
mod tests {

    /// M129 — a custom label survives the decode as a COMPONENT.
    ///
    /// The screen already rendered `Known` through the language table and
    /// `Custom` as a raw flattened string, so one label kind honoured the
    /// player's language and the other did not. Resolution moved to the render
    /// site, which means the decode's job is to not destroy the tree.
    ///
    /// MUTATION: `r.nbt()?.to_plain_text()` (the pre-M129 line, with `Custom`
    /// back to a `String`). The translatable then arrives as its raw key with
    /// its arguments gone, and nothing downstream can recover them.
    #[test]
    fn a_custom_label_keeps_its_component_rather_than_flattening_it() {
        // one entry: Either.right, then a `translate` compound, then the URL.
        let mut body = Vec::new();
        rewo_proto::varint::write_varint(&mut body, 1);
        body.push(0x00); // Either.right -> Custom
        body.push(0x0a); // TAG_Compound
        body.push(0x08); // TAG_String
        body.extend_from_slice(&(9u16).to_be_bytes());
        body.extend_from_slice(b"translate");
        let key = "server.links.discord";
        body.extend_from_slice(&(key.len() as u16).to_be_bytes());
        body.extend_from_slice(key.as_bytes());
        body.push(0x00); // TAG_End
        let url = "https://example.invalid/";
        rewo_proto::varint::write_varint(&mut body, url.len() as i32);
        body.extend_from_slice(url.as_bytes());

        let got = read_server_links(&body).unwrap();
        assert_eq!(got.len(), 1);
        let ServerLinkLabel::Custom(tag) = &got[0].label else {
            panic!("expected a custom label")
        };
        assert_eq!(
            tag.get("translate"),
            Some(&rewo_proto::nbt::Nbt::String(key.to_string())),
            "the tree survives; a flattening decode leaves only the key's text"
        );
        assert_eq!(got[0].link, url);
    }
    use super::*;

    fn wire_string(s: &str) -> Vec<u8> {
        let mut out = Vec::new();
        rewo_proto::varint::write_varint(&mut out, s.len() as i32);
        out.extend_from_slice(s.as_bytes());
        out
    }

    /// A network-NBT string tag — what `fromCodecTrusted` produces for a
    /// plain-text `Component`.
    fn wire_component(text: &str) -> Vec<u8> {
        let mut out = vec![0x08];
        out.extend_from_slice(&(text.len() as u16).to_be_bytes());
        out.extend_from_slice(text.as_bytes());
        out
    }

    fn known(id: i32, url: &str) -> Vec<u8> {
        let mut out = vec![0x01];
        rewo_proto::varint::write_varint(&mut out, id);
        out.extend(wire_string(url));
        out
    }

    fn custom(label: &str, url: &str) -> Vec<u8> {
        let mut out = vec![0x00];
        out.extend(wire_component(label));
        out.extend(wire_string(url));
        out
    }

    /// The `Either` flag: **`true` selects the left**, which is the enum.
    ///
    /// MUTATION: invert the boolean. Both branches then still decode a
    /// well-formed body — a `true` entry would try to read an NBT tag out of a
    /// VarInt id and a `false` entry a VarInt out of a tag byte — so the tell
    /// is not "does it parse" but *which* alternative comes out. The sample
    /// pairs a `Known(Status)` with a `Custom`, so an inversion turns the first
    /// into a decode failure rather than a wrong-but-plausible label.
    #[test]
    fn the_either_flag_is_true_for_the_known_type() {
        let mut body = vec![0x02];
        body.extend(known(3, "https://status.example"));
        body.extend(custom("Our Discord", "https://discord.example"));
        let got = read_server_links(&body).unwrap();
        assert_eq!(
            got,
            vec![
                UntrustedEntry {
                    label: ServerLinkLabel::Known(KnownLinkType::Status),
                    link: "https://status.example".into(),
                },
                UntrustedEntry {
                    label: ServerLinkLabel::Custom(rewo_proto::nbt::Nbt::String("Our Discord".into())),
                    link: "https://discord.example".into(),
                },
            ]
        );
    }

    /// `ByIdMap.continuous(…, ZERO)`: out of range is `BUG_REPORT`, in both
    /// directions, and it is not an error.
    ///
    /// MUTATION: rejecting an out-of-range id (the `readEnum` convention), or
    /// wrapping it (`WRAP`, which would answer `Support` for 12 and
    /// `Announcements` for −1). Both are conventions that really exist one
    /// field away in other packets — M65 found `readEnum` and `ZERO` inside a
    /// single packet, M83 found `WRAP` and `readEnum` one byte apart.
    #[test]
    fn an_out_of_range_link_type_becomes_bug_report_rather_than_an_error() {
        assert_eq!(KnownLinkType::by_id(0), KnownLinkType::BugReport);
        assert_eq!(KnownLinkType::by_id(9), KnownLinkType::Announcements);
        assert_eq!(KnownLinkType::by_id(10), KnownLinkType::BugReport);
        assert_eq!(KnownLinkType::by_id(-1), KnownLinkType::BugReport);
        assert_eq!(KnownLinkType::by_id(i32::MIN), KnownLinkType::BugReport);
        // WRAP would give Support (12 % 10 == 2) and Announcements (−1).
        assert_ne!(KnownLinkType::by_id(12), KnownLinkType::Support);
        assert_ne!(KnownLinkType::by_id(-1), KnownLinkType::Announcements);

        // And through the reader, so the substitution is not merely a helper.
        let mut body = vec![0x01];
        body.extend(known(99, "https://x.example"));
        assert_eq!(
            read_server_links(&body).unwrap()[0].label,
            ServerLinkLabel::Known(KnownLinkType::BugReport)
        );
    }

    /// The ten names, and the transposed one.
    ///
    /// MUTATION: deriving the name from the constant. `BUG_REPORT` is the only
    /// entry whose string is not its constant lowercased, so a derived name
    /// yields `known_server_link.bug_report` — a key the jar does not have, and
    /// therefore a button labelled with its own key.
    #[test]
    fn bug_reports_lang_key_is_report_bug_and_not_bug_report() {
        assert_eq!(
            KnownLinkType::BugReport.lang_key(),
            "known_server_link.report_bug"
        );
        assert_ne!(
            KnownLinkType::BugReport.lang_key(),
            "known_server_link.bug_report"
        );
        // Every other name really is the constant lowercased, which is what
        // makes the exception easy to miss.
        for t in KnownLinkType::VALUES.iter().skip(1) {
            assert_eq!(t.lang_key(), format!("known_server_link.{}", t.name()));
        }
        // The ids round-trip against the declared order.
        for (i, t) in KnownLinkType::VALUES.iter().enumerate() {
            assert_eq!(t.id(), i as i32);
            assert_eq!(KnownLinkType::by_id(i as i32), *t);
        }
    }

    /// The scheme allowlist, sampled on both sides of it.
    ///
    /// MUTATION: accepting any scheme, or accepting a colonless string. The
    /// first is what a "just keep the string" reading gives; the second is what
    /// `split(':').next()` gives on its own, because a colonless string yields
    /// itself and `"https"` alone would then validate as its own scheme.
    #[test]
    fn only_http_and_https_survive_the_trust_step() {
        for ok in [
            "http://a.example",
            "https://a.example/x?y=1",
            "HTTPS://SHOUTY.EXAMPLE",
            "Http://mixed.example",
        ] {
            assert_eq!(validate_untrusted_uri(ok), Some(ok), "{ok}");
        }
        for bad in [
            "ftp://a.example",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "example.com/no-scheme",
            "://empty-scheme",
            "",
            "https",
        ] {
            assert_eq!(validate_untrusted_uri(bad), None, "{bad}");
        }
    }

    /// One bad link drops itself; the rest of the packet still applies.
    ///
    /// MUTATION: failing the whole packet on a bad entry. That is the instinct
    /// M41 records for an untranscribed component — and it is wrong here,
    /// because `handleServerLinks` catches **inside** the loop, so a server one
    /// typo away from a broken link keeps every other link it advertised.
    #[test]
    fn an_invalid_link_drops_itself_and_the_rest_survive() {
        let mut body = vec![0x03];
        body.extend(known(0, "https://bugs.example"));
        body.extend(known(2, "ftp://support.example"));
        body.extend(custom("Shop", "http://shop.example"));
        let (links, dropped) = trust(read_server_links(&body).unwrap());
        assert_eq!(dropped, 1);
        assert_eq!(links.len(), 2);
        assert_eq!(links.entries()[0].link, "https://bugs.example");
        assert_eq!(links.entries()[1].link, "http://shop.example");
    }

    /// `findKnownType` matches the left alternative only, and takes the first.
    ///
    /// MUTATION: matching a custom label by its text, or taking the last.
    /// The first is what `r -> false` forbids and it is the plausible one — a
    /// custom label reading "Report a bug" is *not* a `BUG_REPORT` entry, and
    /// treating it as one would put an arbitrary server string on the
    /// disconnect screen's report button.
    #[test]
    fn find_known_type_ignores_custom_labels_and_takes_the_first_match() {
        let links = ServerLinks::new(vec![
            ServerLink {
                label: ServerLinkLabel::Custom(rewo_proto::nbt::Nbt::String("Report a bug".into())),
                link: "https://decoy.example".into(),
            },
            ServerLink {
                label: ServerLinkLabel::Known(KnownLinkType::BugReport),
                link: "https://first.example".into(),
            },
            ServerLink {
                label: ServerLinkLabel::Known(KnownLinkType::BugReport),
                link: "https://second.example".into(),
            },
        ]);
        let found = links.find_known_type(KnownLinkType::BugReport).unwrap();
        assert_eq!(found.link, "https://first.example");
        assert_eq!(links.find_known_type(KnownLinkType::Status), None);
        assert!(!links.is_empty());
        assert!(ServerLinks::default().is_empty());
        assert_eq!(
            ServerLinks::default().find_known_type(KnownLinkType::BugReport),
            None
        );
    }

    /// An empty list is a legal packet — and it is the one a server sends to
    /// *retract* its links, because `handleServerLinks` assigns rather than
    /// merges.
    #[test]
    fn an_empty_link_list_is_a_legal_packet() {
        assert!(read_server_links(&[0x00]).unwrap().is_empty());
        // A truncated body is not.
        assert!(read_server_links(&[0x02, 0x01]).is_err());
        assert!(read_server_links(&[]).is_err());
    }
}
