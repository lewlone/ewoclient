//! The tab list's header and footer — `ClientboundTabListPacket` (M65).
//!
//! **Decode + state only.** `rewo_gpu::tab_list` (M52f) already models the
//! *rows*; this is the two blocks of text above and below them, which is the
//! last piece of tab-list state the wire carries and Rewo was dropping.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/network/protocol/game/ClientboundTabListPacket.java` — a
//!   two-field composite, both `ComponentSerialization.TRUSTED_STREAM_CODEC`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` —
//!   `handleTabListCustomisation`, which is where the one interesting rule is
//!
//! ## The rule that is not in the packet
//!
//! ```java
//! tabList.setHeader(packet.header().getString().isEmpty() ? null : packet.header());
//! ```
//!
//! The packet **always carries two components** — there is no optional field —
//! and "no header" is expressed as a component whose *flattened text* is
//! empty. So the emptiness test is on the rendered string, not on the tag:
//! `{"text":"","extra":[{"text":"hi"}]}` has an empty `text` and a non-empty
//! `getString()`, and it is a header.
//!
//! Rewo flattens with [`crate::chat_style`], which resolves `text` and
//! `extra` and emits a `translate` key verbatim. **Two documented gaps**, both
//! erring the same way: a `keybind` / `score` / `selector` / `nbt` component
//! flattens to nothing here where vanilla resolves it to real text, so such a
//! header would be dropped as empty. No vanilla server sends one; a plugin
//! could. The alternative — testing the tag for emptiness instead — gets the
//! ordinary `{"text":""}` case wrong, which every server does send.

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

use crate::chat_style;

/// The decoded packet: the two components exactly as they arrived.
#[derive(Clone, Debug, PartialEq)]
pub struct TabListPacket {
    pub header: Nbt,
    pub footer: Nbt,
}

impl TabListPacket {
    pub fn read(r: &mut PacketReader<'_>) -> Result<TabListPacket> {
        Ok(TabListPacket {
            header: r.nbt()?,
            footer: r.nbt()?,
        })
    }
}

pub fn parse_tab_list(body: &[u8]) -> Result<TabListPacket> {
    TabListPacket::read(&mut PacketReader::new(body))
}

/// True when a component renders as no text at all — `Component.getString()
/// .isEmpty()`.
///
/// The base style decides colours and nothing else, and this reads only the
/// characters, so any base does — white is passed because it is the surface
/// default a caller rendering the header would use.
///
/// **No language table** (M125): this runs at decode time, where there is
/// none, so a `translate` component counts as its key and is therefore never
/// empty. Vanilla's `getString()` resolves through the `Language.getInstance()`
/// global and so would answer `true` for a key whose translation is the empty
/// string. Named rather than hidden; no vanilla template is empty, so the two
/// readings differ only for a server that ships one deliberately.
pub fn renders_empty(component: &Nbt) -> bool {
    // [`chat_style::flatten`]'s base IS `ChatStyle::plain([1.0, 1.0, 1.0])` —
    // `ChatStyle::WHITE` is defined as exactly that constant — so delegating
    // here is the identity, not an approximation. It is done because the
    // open-coded form was one of four spellings of one walk (M161's census).
    chat_style::flatten(component, None).is_empty()
}

/// The tab list's header and footer as the client holds them
/// (`PlayerTabOverlay.header` / `.footer`).
///
/// `None` is "draw nothing", which is a different state from "draw an empty
/// line" — vanilla's renderer skips the block entirely rather than reserving
/// its height.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabListText {
    pub header: Option<Nbt>,
    pub footer: Option<Nbt>,
}

impl TabListText {
    pub fn new() -> TabListText {
        TabListText::default()
    }

    /// `handleTabListCustomisation` — both fields are assigned every time, so
    /// a packet with an empty header **clears** a header set by an earlier
    /// one.
    pub fn apply(&mut self, p: &TabListPacket) {
        self.header = Some(p.header.clone()).filter(|c| !renders_empty(c));
        self.footer = Some(p.footer.clone()).filter(|c| !renders_empty(c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: u8 = 0xA7;

    fn nbt_string(out: &mut Vec<u8>, s: &str) {
        out.push(8);
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn body(header: &str, footer: &str) -> Vec<u8> {
        let mut b = Vec::new();
        nbt_string(&mut b, header);
        nbt_string(&mut b, footer);
        b
    }

    /// A compound `{"text": text, "extra": [{"text": extra}]}`, network-NBT.
    fn nbt_text_with_extra(text: &str, extra: &str) -> Nbt {
        Nbt::Compound(vec![
            ("text".into(), Nbt::String(text.into())),
            (
                "extra".into(),
                Nbt::List(vec![Nbt::Compound(vec![(
                    "text".into(),
                    Nbt::String(extra.into()),
                )])]),
            ),
        ])
    }

    #[test]
    fn the_packet_is_two_components_and_consumes_exactly_them() {
        let mut b = body("Welcome", "Bye");
        b.push(SENTINEL);
        let mut r = PacketReader::new(&b);
        let p = TabListPacket::read(&mut r).unwrap();
        assert_eq!(r.remaining(), 1, "decode must stop at the sentinel");
        assert_eq!(r.u8().unwrap(), SENTINEL);
        assert_eq!(p.header, Nbt::String("Welcome".into()));
        assert_eq!(p.footer, Nbt::String("Bye".into()));
    }

    #[test]
    fn a_truncated_body_is_an_error_rather_than_a_header_with_no_footer() {
        let full = body("Welcome", "Bye");
        assert!(parse_tab_list(&full[..full.len() - 1]).is_err());
    }

    #[test]
    fn an_empty_component_means_no_header_rather_than_an_empty_one() {
        let mut text = TabListText::new();
        text.apply(&parse_tab_list(&body("Welcome", "")).unwrap());
        assert_eq!(text.header, Some(Nbt::String("Welcome".into())));
        assert_eq!(text.footer, None);
    }

    #[test]
    fn a_later_empty_component_clears_what_an_earlier_one_set() {
        // Both fields are assigned unconditionally, so this is how a server
        // takes a header down.
        let mut text = TabListText::new();
        text.apply(&parse_tab_list(&body("Welcome", "Bye")).unwrap());
        assert!(text.header.is_some() && text.footer.is_some());
        text.apply(&parse_tab_list(&body("", "")).unwrap());
        assert_eq!(text, TabListText::default());
    }

    #[test]
    fn emptiness_is_of_the_rendered_text_not_of_the_text_field() {
        // The invertible one. A component with an empty `text` and a non-empty
        // `extra` renders as text, so it IS a header — testing `text` alone
        // would drop it.
        let component = nbt_text_with_extra("", "Welcome");
        assert!(!renders_empty(&component));

        let empty_all_the_way = nbt_text_with_extra("", "");
        assert!(renders_empty(&empty_all_the_way));

        let mut text = TabListText::new();
        text.apply(&TabListPacket {
            header: component.clone(),
            footer: empty_all_the_way,
        });
        assert_eq!(text.header, Some(component));
        assert_eq!(text.footer, None);
    }

    #[test]
    fn a_legacy_colour_code_alone_still_renders_empty() {
        // `§c` produces no characters, so `getString()` is empty and vanilla
        // stores null. `chat_style` drops the zero-length span for the same
        // reason, so the two agree.
        assert!(renders_empty(&Nbt::String("\u{a7}c".into())));
        assert!(!renders_empty(&Nbt::String("\u{a7}cRed".into())));
    }
}
