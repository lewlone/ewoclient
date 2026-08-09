//! `ClickEvent`, `HoverEvent` and `insertion` — the three `Style` fields that
//! make chat text *do* something (M128).
//!
//! # Why they were dropped until now
//!
//! [`crate::chat_style::parse_component`] read `color` and the five format
//! flags and stopped there, because until M126 a chat line was a `String` and
//! there was nowhere to put anything else. There is now, and these are what
//! `Style` carries beyond the visual fields:
//!
//! ```java
//! ClickEvent.CODEC.optionalFieldOf("click_event").forGetter(o -> Optional.ofNullable(o.clickEvent)),
//! HoverEvent.CODEC.optionalFieldOf("hover_event").forGetter(o -> Optional.ofNullable(o.hoverEvent)),
//! Codec.STRING.optionalFieldOf("insertion").forGetter(o -> Optional.ofNullable(o.insertion)),
//! ```
//!
//! The field names are **snake_case** (`click_event`, not the pre-1.21.5
//! `clickEvent`), and each event's payload is **inlined beside its `action`**
//! rather than nested — `ClickEvent.CODEC` is
//! `Action.CODEC.dispatch("action", …)`, and `dispatch` puts the dispatched
//! codec's fields in the same map as the key. So a run-command event is
//! `{"action": "run_command", "command": "/kill"}`, one compound, two keys.
//!
//! # Two refusals, both load-bearing, both at DECODE
//!
//! **`open_file` can never arrive from a server.**
//! `ClickEvent.Action.CODEC` is `UNSAFE_CODEC.validate(filterForSerialization)`,
//! `filterForSerialization` errors on any action whose `allowFromServer` is
//! false, and `OPEN_FILE("open_file", false, …)` is the only one. `Codec
//! .validate(checker)` is `flatXmap(checker, checker)` — **the checker runs on
//! decode as well as on encode** (`flatXmap`'s decode half is
//! `this.flatMap(to)`), and it runs on the dispatch *key*, so the payload codec
//! is never even reached. [`ClickEvent`] therefore has no `OpenFile` variant:
//! it is not representable, rather than representable and refused later.
//!
//! **A non-`http(s)` `open_url` is refused the same way**, one layer down.
//! `OpenUrl`'s field codec is `ExtraCodecs.UNTRUSTED_URI`, which runs
//! `Util.parseAndValidateUntrustedUri` — a scheme is required and, lowercased,
//! must be in `ALLOWED_UNTRUSTED_LINK_PROTOCOLS = Set.of("http", "https")`.
//! That is a *decode-time* gate in vanilla, and it is a decode-time gate here:
//! a `file:` or `javascript:` URL never becomes a [`ClickEvent::OpenUrl`], so
//! no consumer can be the place that forgot to check.
//!
//! # The deliberate deviation, stated rather than implied
//!
//! `Style.Serializer.MAP_CODEC` uses **`optionalFieldOf`, the strict one**:
//! DFU 10.0.21's `Codec.optionalFieldOf(name)` is `optionalField(name, this,
//! false)` and `OptionalFieldCodec.decode` returns `Optional.empty()` for an
//! error *only* when `lenient`. So in vanilla a malformed `click_event` does
//! not drop the field — it fails the `Style` decode, which fails the
//! `Component` decode, which reaches
//! `ByteBufCodecs.fromCodecWithRegistries`'s `getOrThrow` and throws a
//! `DecoderException`. **A hostile server sending `open_file` in a chat
//! message disconnects the vanilla client.**
//!
//! Rewo instead **drops the field and keeps the message**. That is a real
//! divergence and not a claim of parity: a chat line that loses a link beats a
//! connection that dies, and it is the same choice
//! [`crate::chat_style::parse_color`] already documents for an unparseable
//! `color`. It is also strictly safer, because the refusal is what remains.
//!
//! # What is decoded and what is only recognised
//!
//! Every [`ClickEvent`] variant vanilla can send is decoded. [`HoverEvent`]'s
//! `show_text` is decoded into its component; `show_item` and `show_entity`
//! are **recognised and carried raw**, because rendering them needs an item
//! tooltip and an entity registry lookup that this module has no business
//! reaching for — and because `show_entity`'s tooltip is suppressed unless
//! `advancedItemTooltips` is on, which is off by default, so it is invisible
//! in vanilla too most of the time.

use std::sync::Arc;

use rewo_proto::nbt::Nbt;

/// `net.minecraft.network.chat.ClickEvent`.
///
/// **There is no `OpenFile`.** See the module docs: the action codec refuses
/// it on decode, so a server-sent one is not a variant this type can hold.
#[derive(Clone, Debug, PartialEq)]
pub enum ClickEvent {
    /// `ClickEvent.OpenUrl(URI uri)`, field `url`.
    ///
    /// Already gated: the scheme is present and is `http` or `https`. Stored
    /// as the string the server sent, because that is what
    /// `Util.OS.openUri` hands to the platform (`URI::toString`).
    OpenUrl(String),
    /// `ClickEvent.RunCommand(String command)`, field `command`, validated by
    /// `ExtraCodecs.CHAT_STRING`.
    ///
    /// The stored string is **as received**, still carrying any leading `/`.
    /// `Commands.trimOptionalPrefix` is applied at the click, not here, so
    /// this stays a faithful record of the packet.
    RunCommand(String),
    /// `ClickEvent.SuggestCommand(String command)`, field `command`, same
    /// validation. Puts the text in the chat input; does not send it.
    SuggestCommand(String),
    /// `ClickEvent.ShowDialog(Holder<Dialog> dialog)`, field `dialog`.
    ///
    /// Carried raw: `Dialog.CODEC` is a whole registry-dispatched screen
    /// description and Rewo has no dialog screen.
    ShowDialog(Nbt),
    /// `ClickEvent.ChangePage(int page)`, field `page`, `ExtraCodecs
    /// .POSITIVE_INT` — so zero and negatives are refused, not clamped.
    ChangePage(i32),
    /// `ClickEvent.CopyToClipboard(String value)`, field `value`, plain
    /// `Codec.STRING` — **not** `CHAT_STRING`, so a section sign is legal here
    /// and illegal in a command.
    CopyToClipboard(String),
    /// `ClickEvent.Custom(Identifier id, Optional<Tag> payload)`.
    ///
    /// The id is validated as an `Identifier` (namespace `[a-z0-9_.-]`, path
    /// `[a-z0-9_.-/]`) because that is what refusing it costs, and because two
    /// of vanilla's own chat affordances — the delayed-message expand link and
    /// the restrictions screen — are `Custom` events matched *by id*.
    Custom { id: String, payload: Option<Nbt> },
}

impl ClickEvent {
    /// `ClickEvent.Action.getSerializedName()` — the string the server sent.
    ///
    /// Exists so a decline can name the action without a second table.
    pub fn action_name(&self) -> &'static str {
        match self {
            ClickEvent::OpenUrl(_) => "open_url",
            ClickEvent::RunCommand(_) => "run_command",
            ClickEvent::SuggestCommand(_) => "suggest_command",
            ClickEvent::ShowDialog(_) => "show_dialog",
            ClickEvent::ChangePage(_) => "change_page",
            ClickEvent::CopyToClipboard(_) => "copy_to_clipboard",
            ClickEvent::Custom { .. } => "custom",
        }
    }
}

/// `net.minecraft.network.chat.HoverEvent`.
///
/// All three actions have `allowFromServer = true`, so unlike [`ClickEvent`]
/// the `validate` here is a no-op on decode — recorded because the two enums
/// look identical and only one of them refuses anything.
#[derive(Clone, Debug, PartialEq)]
pub enum HoverEvent {
    /// `HoverEvent.ShowText(Component value)`, field `value`, a full
    /// recursive component — so it resolves through
    /// [`crate::chat_style::parse_component`] like any other.
    ShowText(Nbt),
    /// `HoverEvent.ShowItem(ItemStackTemplate item)`.
    ///
    /// `ShowItem.CODEC` is `ItemStackTemplate.MAP_CODEC.xmap(…)`, so `id`,
    /// `count` and `components` sit in the **same compound as `action`** —
    /// there is no `contents` wrapper (that is the pre-1.21.5 shape). Carried
    /// as that whole compound, `action` included, because Rewo does not decode
    /// it and a partial decode would be a lie about what it holds.
    ShowItem(Nbt),
    /// `HoverEvent.ShowEntity(EntityTooltipInfo entity)` — `id`, `uuid` and an
    /// optional `name`, also inlined beside `action`. Carried raw for the same
    /// reason, and additionally because vanilla draws it **only when
    /// `advancedItemTooltips`** is on (F3+H), which is off by default.
    ShowEntity(Nbt),
}

impl HoverEvent {
    pub fn action_name(&self) -> &'static str {
        match self {
            HoverEvent::ShowText(_) => "show_text",
            HoverEvent::ShowItem(_) => "show_item",
            HoverEvent::ShowEntity(_) => "show_entity",
        }
    }
}

/// The three non-visual `Style` fields, together.
///
/// Grouped into one struct so [`crate::chat_style::ChatStyle`] grows **one**
/// field rather than three, but merged **field by field** on inherit — see
/// [`ChatEvents::apply_to`]. That distinction is the whole reason this is not
/// simply an `Option<Arc<…>>` swapped wholesale: `Style.applyTo` is
/// `this.clickEvent != null ? this.clickEvent : other.clickEvent` *per field*,
/// so a child carrying only a `hover_event` keeps its parent's `click_event`.
/// Replacing the group would silently un-link the text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatEvents {
    pub click: Option<ClickEvent>,
    pub hover: Option<HoverEvent>,
    /// `Style.insertion` — shift-clicked into the chat input. Plain
    /// `Codec.STRING`, no validation.
    pub insertion: Option<String>,
}

impl ChatEvents {
    pub fn is_empty(&self) -> bool {
        self.click.is_none() && self.hover.is_none() && self.insertion.is_none()
    }

    /// `Style.applyTo(parent)` for these three fields — present wins, absent
    /// inherits, **independently**.
    ///
    /// `self` is the child's own patch; `parent` is the enclosing resolved
    /// value. Returns the parent's `Arc` unchanged when the child says
    /// nothing, so the common case allocates nothing.
    pub fn apply_to(
        child: Option<&ChatEvents>,
        parent: Option<&Arc<ChatEvents>>,
    ) -> Option<Arc<ChatEvents>> {
        let Some(child) = child.filter(|c| !c.is_empty()) else {
            return parent.cloned();
        };
        let Some(parent) = parent else {
            return Some(Arc::new(child.clone()));
        };
        Some(Arc::new(ChatEvents {
            click: child.click.clone().or_else(|| parent.click.clone()),
            hover: child.hover.clone().or_else(|| parent.hover.clone()),
            insertion: child
                .insertion
                .clone()
                .or_else(|| parent.insertion.clone()),
        }))
    }
}

/// Read the three fields off a component compound, as
/// `Style.Serializer.MAP_CODEC` reads them.
///
/// `None` when the compound carries none of them — which is the overwhelming
/// majority of components, and is why this returns an `Option` rather than an
/// empty struct.
pub fn parse_events(tag: &Nbt) -> Option<ChatEvents> {
    let events = ChatEvents {
        click: tag.get("click_event").and_then(parse_click_event),
        hover: tag.get("hover_event").and_then(parse_hover_event),
        insertion: tag
            .get("insertion")
            .and_then(Nbt::as_str)
            .map(str::to_owned),
    };
    (!events.is_empty()).then_some(events)
}

/// `ClickEvent.CODEC` — `Action.CODEC.dispatch("action", …)`.
///
/// Every failure is `None`: an unknown action, a refused one, a missing or
/// mistyped payload field, or a payload that fails its own codec. Vanilla
/// errors instead; see the module docs.
pub fn parse_click_event(tag: &Nbt) -> Option<ClickEvent> {
    let action = tag.get("action").and_then(Nbt::as_str)?;
    Some(match action {
        // `ExtraCodecs.UNTRUSTED_URI`.
        "open_url" => ClickEvent::OpenUrl(parse_untrusted_uri(
            tag.get("url").and_then(Nbt::as_str)?,
        )?),
        // `allowFromServer = false`. The action codec's `validate` rejects the
        // dispatch key before any payload is read, so this is not "decoded and
        // then refused" — it never decodes.
        "open_file" => return None,
        "run_command" => {
            ClickEvent::RunCommand(chat_string(tag.get("command").and_then(Nbt::as_str)?)?)
        }
        "suggest_command" => {
            ClickEvent::SuggestCommand(chat_string(tag.get("command").and_then(Nbt::as_str)?)?)
        }
        "show_dialog" => ClickEvent::ShowDialog(tag.get("dialog")?.clone()),
        // `ExtraCodecs.POSITIVE_INT`.
        "change_page" => {
            let page = nbt_i32(tag.get("page")?)?;
            if page <= 0 {
                return None;
            }
            ClickEvent::ChangePage(page)
        }
        "copy_to_clipboard" => {
            ClickEvent::CopyToClipboard(tag.get("value").and_then(Nbt::as_str)?.to_owned())
        }
        "custom" => ClickEvent::Custom {
            id: parse_identifier(tag.get("id").and_then(Nbt::as_str)?)?,
            payload: tag.get("payload").cloned(),
        },
        _ => return None,
    })
}

/// `HoverEvent.CODEC` — the same dispatch, with nothing refused by the action
/// filter.
pub fn parse_hover_event(tag: &Nbt) -> Option<HoverEvent> {
    let action = tag.get("action").and_then(Nbt::as_str)?;
    Some(match action {
        "show_text" => HoverEvent::ShowText(tag.get("value")?.clone()),
        "show_item" => HoverEvent::ShowItem(tag.clone()),
        "show_entity" => HoverEvent::ShowEntity(tag.clone()),
        _ => return None,
    })
}

/// `Util.parseAndValidateUntrustedUri` — **the security gate**, transcribed.
///
/// ```java
/// URI parsedUri = new URI(uri);
/// String scheme = parsedUri.getScheme();
/// if (scheme == null) throw …;
/// String protocol = scheme.toLowerCase(Locale.ROOT);
/// if (!ALLOWED_UNTRUSTED_LINK_PROTOCOLS.contains(protocol)) throw …;
/// ```
///
/// with `ALLOWED_UNTRUSTED_LINK_PROTOCOLS = Set.of("http", "https")`.
///
/// The scheme grammar is RFC 2396's — `ALPHA *( ALPHA | DIGIT | "+" | "-" |
/// "." )` up to the first `:` — and a leading `:` or a first segment that is
/// not a legal scheme means `getScheme()` is null (the URI is then a relative
/// reference), which is the `scheme == null` throw.
///
/// **The one deviation, stated:** `new URI(String)` also rejects every
/// character outside RFC 2396's grammar, which is a much larger check than
/// "the scheme is http". Rewo does not reproduce `java.net.URI`'s full
/// syntax analysis; it requires every byte to be an ASCII graphic character
/// (`0x21..=0x7E`), which refuses the space, the control characters and all
/// non-ASCII that Java also refuses, and accepts a handful of malformed URIs
/// Java would not. Nothing downstream is a shell, so the residue is a URL the
/// platform opener will fail to resolve rather than a way to run something.
pub fn parse_untrusted_uri(uri: &str) -> Option<String> {
    if uri.is_empty() || !uri.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
        return None;
    }
    let scheme = uri_scheme(uri)?;
    let protocol = scheme.to_ascii_lowercase();
    if protocol != "http" && protocol != "https" {
        return None;
    }
    Some(uri.to_owned())
}

/// `URI.getScheme()` for an absolute URI — `None` when the string is a
/// relative reference, which is what makes `scheme == null` reachable.
///
/// **The two character rules below are unobservable through
/// [`parse_untrusted_uri`], and provably so**: the only schemes that survive
/// `ALLOWED_UNTRUSTED_LINK_PROTOCOLS` are `http` and `https`, both of which
/// already satisfy them, so no input exists for which they decide the answer.
/// They are transcribed because they are RFC 2396's and because the whitelist
/// is one line away from changing, and they are tested **here rather than
/// through the public entry point** — a mutation battery on the caller
/// correctly reports them as equivalent.
fn uri_scheme(uri: &str) -> Option<&str> {
    let colon = uri.find(':')?;
    let scheme = &uri[..colon];
    let mut chars = scheme.chars();
    // "An implementation should accept uppercase letters as equivalent to
    // lowercase" — but the FIRST character must still be a letter, so `1a:` is
    // not a scheme and `//host` has none at all.
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    chars
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        .then_some(scheme)
}

/// `ExtraCodecs.CHAT_STRING` — `StringUtil.isAllowedChatCharacter` over every
/// character.
///
/// ```java
/// return ch != 167 && ch >= 32 && ch != 127;
/// ```
///
/// 167 is the section sign, so a command carrying a `§` is refused where a
/// `copy_to_clipboard` value carrying one is fine — the two use different
/// codecs and it is easy to give them the same one.
pub fn chat_string(s: &str) -> Option<String> {
    s.chars()
        .all(|c| c != '\u{00A7}' && c >= ' ' && c != '\u{007F}')
        .then(|| s.to_owned())
}

/// `Identifier.parse` / `read` — `namespace:path`, or a bare path with the
/// `minecraft` namespace implied.
///
/// `validNamespaceChar` is `[a-z0-9_.-]` and `validPathChar` is the same plus
/// `/`. Returned in canonical `namespace:path` form so a caller comparing
/// against `"minecraft:…"` matches a server that wrote the bare path.
pub fn parse_identifier(s: &str) -> Option<String> {
    let (namespace, path) = match s.find(':') {
        // `bySeparator`: `separatorIndex != 0` decides whether the namespace is
        // taken from the text before the colon or defaulted, so a LEADING
        // colon is `minecraft:` plus the rest, not an empty namespace.
        Some(0) => ("minecraft", &s[1..]),
        Some(i) => (&s[..i], &s[i + 1..]),
        None => ("minecraft", s),
    };
    let ns_ok = namespace != ".."
        && namespace
            .chars()
            .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-' | '.'));
    let path_ok = path
        .chars()
        .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-' | '.' | '/'));
    (ns_ok && path_ok).then(|| format!("{namespace}:{path}"))
}

/// `Codec.INT` through `NbtOps`, which reads any numeric tag.
fn nbt_i32(tag: &Nbt) -> Option<i32> {
    Some(match tag {
        Nbt::Byte(v) => i32::from(*v),
        Nbt::Short(v) => i32::from(*v),
        Nbt::Int(v) => *v,
        Nbt::Long(v) => *v as i32,
        Nbt::Float(v) => *v as i32,
        Nbt::Double(v) => *v as i32,
        _ => return None,
    })
}

/// `GuiGraphicsExtractor.componentHoverEffect`'s `show_text` arm, resolved and
/// wrapped — the lines a tooltip would draw.
///
/// ```java
/// case HoverEvent.ShowText(Component text):
///    this.setTooltipForNextFrame(font, font.split(text, Math.max(this.guiWidth() / 2, 200)), xMouse, yMouse);
/// ```
///
/// `Font.split` is `splitter.splitLines(input, maxWidth, Style.EMPTY)` — the
/// same overload the chat wrap uses, **without** `wrapComponents`' indent, so
/// a continuation here is not prefixed with a space.
///
/// The width is [`hover_text_width`], and `Style.EMPTY` is why the base style
/// is white-with-no-flags rather than whatever the hovered run carried: the
/// tooltip's text is the hover event's own component, resolved from scratch.
///
/// **Only `show_text` resolves.** `show_item` needs an item tooltip and
/// `show_entity` needs an entity registry *and* is suppressed unless
/// `advancedItemTooltips` is on; both answer `None` here rather than a
/// plausible wrong line.
pub fn show_text_lines(
    style: &crate::chat_style::ChatStyle,
    lang: Option<&rewo_data::lang::Language>,
    gui_width: i32,
    width_of: &dyn Fn(&str, crate::chat_style::ChatStyle) -> i32,
) -> Option<Vec<crate::chat_style::ChatLine>> {
    let HoverEvent::ShowText(value) = style.hover()? else {
        return None;
    };
    let spans = crate::chat_style::parse_component(
        value,
        crate::chat_style::ChatStyle::WHITE,
        lang,
    );
    Some(
        crate::string_splitter::split_lines_wrapped(&spans, hover_text_width(gui_width), width_of)
            .into_iter()
            .map(|l| l.spans)
            .collect(),
    )
}

/// `Math.max(this.guiWidth() / 2, 200)` — an **integer** halving, and a floor
/// of 200 rather than a clamp, so a narrow window still gets a readable
/// tooltip and a wide one gets half of it.
pub fn hover_text_width(gui_width: i32) -> i32 {
    (gui_width / 2).max(200)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compound(fields: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    fn s(v: &str) -> Nbt {
        Nbt::String(v.into())
    }

    fn click(fields: &[(&str, Nbt)]) -> Option<ClickEvent> {
        parse_click_event(&compound(fields))
    }

    /// The payload sits BESIDE `action`, not under a `value` wrapper — that is
    /// `dispatch`, and it is the shape a pre-1.21.5 transcription gets wrong.
    #[test]
    fn the_payload_is_inlined_beside_the_action() {
        assert_eq!(
            click(&[("action", s("run_command")), ("command", s("/kill"))]),
            Some(ClickEvent::RunCommand("/kill".into()))
        );
        // Nested is not the shape, so it decodes to nothing.
        assert_eq!(
            click(&[
                ("action", s("run_command")),
                ("value", compound(&[("command", s("/kill"))]))
            ]),
            None
        );
    }

    /// `OPEN_FILE("open_file", false, …)` is the only action whose
    /// `allowFromServer` is false, and `Action.CODEC`'s `validate` runs on
    /// decode.
    #[test]
    fn open_file_is_refused() {
        assert_eq!(
            click(&[("action", s("open_file")), ("path", s("C:/windows"))]),
            None
        );
    }

    /// `ALLOWED_UNTRUSTED_LINK_PROTOCOLS = Set.of("http", "https")`, and the
    /// gate is on the SCHEME, at decode.
    #[test]
    fn only_http_and_https_urls_decode() {
        let url = |u: &str| click(&[("action", s("open_url")), ("url", s(u))]);
        assert_eq!(
            url("https://example.com/a?b=c#d"),
            Some(ClickEvent::OpenUrl("https://example.com/a?b=c#d".into()))
        );
        assert_eq!(url("http://example.com"), Some(ClickEvent::OpenUrl("http://example.com".into())));
        // `toLowerCase(Locale.ROOT)` on the scheme, so the case is not a gate.
        assert_eq!(url("HTTPS://example.com"), Some(ClickEvent::OpenUrl("HTTPS://example.com".into())));
        for bad in [
            "file:///C:/windows/system32",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "ftp://example.com",
            "steam://run/1",
            // `getScheme()` is null for a relative reference.
            "//example.com",
            "example.com",
            "/etc/passwd",
            // A leading colon leaves an empty first segment, so there is no
            // scheme at all.
            ":http://example.com",
            // A scheme must START with a letter.
            "1http://example.com",
            "",
        ] {
            assert_eq!(url(bad), None, "{bad} must not decode");
        }
    }

    /// A space is outside `new URI(String)`'s grammar, and so are the control
    /// characters and every non-ASCII byte.
    #[test]
    fn a_url_with_a_space_or_a_control_character_does_not_decode() {
        let url = |u: &str| click(&[("action", s("open_url")), ("url", s(u))]);
        assert_eq!(url("http://example.com/a b"), None);
        assert_eq!(url("http://example.com/\u{0007}"), None);
        assert_eq!(url("http://exämple.com"), None);
    }

    /// `CHAT_STRING` is `ch != 167 && ch >= 32 && ch != 127`, and it is on the
    /// two command actions only.
    #[test]
    fn a_command_may_not_carry_a_section_sign_and_a_clipboard_value_may() {
        assert_eq!(
            click(&[("action", s("run_command")), ("command", s("/say \u{00A7}c"))]),
            None
        );
        assert_eq!(
            click(&[("action", s("suggest_command")), ("command", s("/say \u{00A7}c"))]),
            None
        );
        assert_eq!(
            click(&[("action", s("run_command")), ("command", s("/say \u{0007}"))]),
            None
        );
        // Plain `Codec.STRING`.
        assert_eq!(
            click(&[("action", s("copy_to_clipboard")), ("value", s("\u{00A7}c"))]),
            Some(ClickEvent::CopyToClipboard("\u{00A7}c".into()))
        );
    }

    /// The command keeps its slash: `trimOptionalPrefix` belongs to the click.
    #[test]
    fn run_command_keeps_the_leading_slash() {
        assert_eq!(
            click(&[("action", s("run_command")), ("command", s("/kill @e"))]),
            Some(ClickEvent::RunCommand("/kill @e".into()))
        );
    }

    /// `ExtraCodecs.POSITIVE_INT` — refused, not clamped.
    #[test]
    fn change_page_must_be_positive() {
        let page = |n: i32| click(&[("action", s("change_page")), ("page", Nbt::Int(n))]);
        assert_eq!(page(1), Some(ClickEvent::ChangePage(1)));
        assert_eq!(page(0), None);
        assert_eq!(page(-3), None);
    }

    #[test]
    fn an_unknown_action_decodes_to_nothing() {
        assert_eq!(click(&[("action", s("open_book"))]), None);
        assert_eq!(click(&[("command", s("/kill"))]), None);
    }

    /// `Identifier` validation, including the two shapes that are easy to get
    /// backwards: a bare path defaults the namespace, and a LEADING colon does
    /// too rather than producing an empty one.
    #[test]
    fn custom_ids_are_identifiers() {
        let custom = |id: &str| click(&[("action", s("custom")), ("id", s(id))]);
        assert_eq!(
            custom("minecraft:chat_expand"),
            Some(ClickEvent::Custom { id: "minecraft:chat_expand".into(), payload: None })
        );
        assert_eq!(
            custom("chat_expand"),
            Some(ClickEvent::Custom { id: "minecraft:chat_expand".into(), payload: None })
        );
        assert_eq!(
            custom(":chat_expand"),
            Some(ClickEvent::Custom { id: "minecraft:chat_expand".into(), payload: None })
        );
        // `validNamespaceChar` has no `/`; `validPathChar` does.
        assert_eq!(
            custom("a/b:c"),
            None
        );
        assert_eq!(
            custom("ns:a/b"),
            Some(ClickEvent::Custom { id: "ns:a/b".into(), payload: None })
        );
        assert_eq!(custom("Upper:case"), None);
        assert_eq!(custom("..:x"), None);
    }

    #[test]
    fn custom_carries_its_payload() {
        assert_eq!(
            click(&[
                ("action", s("custom")),
                ("id", s("x:y")),
                ("payload", Nbt::Int(7)),
            ]),
            Some(ClickEvent::Custom { id: "x:y".into(), payload: Some(Nbt::Int(7)) })
        );
    }

    /// `show_item` and `show_entity` inline their fields beside `action`, so
    /// the carried tag is the whole compound; `show_text`'s is its `value`.
    #[test]
    fn hover_events_decode() {
        let hover = |fields: &[(&str, Nbt)]| parse_hover_event(&compound(fields));
        assert_eq!(
            hover(&[("action", s("show_text")), ("value", s("hi"))]),
            Some(HoverEvent::ShowText(s("hi")))
        );
        let item = [("action", s("show_item")), ("id", s("minecraft:stone"))];
        assert_eq!(hover(&item), Some(HoverEvent::ShowItem(compound(&item))));
        let entity = [("action", s("show_entity")), ("id", s("minecraft:pig"))];
        assert_eq!(hover(&entity), Some(HoverEvent::ShowEntity(compound(&entity))));
        assert_eq!(hover(&[("action", s("show_achievement"))]), None);
    }

    /// `parse_events` answers `None` for the overwhelming majority of
    /// components — the ones carrying none of the three fields.
    #[test]
    fn a_component_with_no_events_carries_none() {
        assert_eq!(parse_events(&compound(&[("text", s("hi"))])), None);
        assert_eq!(
            parse_events(&compound(&[("text", s("hi")), ("insertion", s("Steve"))])),
            Some(ChatEvents { click: None, hover: None, insertion: Some("Steve".into()) })
        );
    }

    /// **`applyTo` is per field.** A child carrying only a hover keeps the
    /// parent's click — merging the group wholesale would un-link the text and
    /// look perfectly reasonable.
    #[test]
    fn inherit_is_field_by_field() {
        let parent = Arc::new(ChatEvents {
            click: Some(ClickEvent::RunCommand("/a".into())),
            hover: None,
            insertion: Some("p".into()),
        });
        let child = ChatEvents {
            click: None,
            hover: Some(HoverEvent::ShowText(s("t"))),
            insertion: None,
        };
        let merged = ChatEvents::apply_to(Some(&child), Some(&parent)).unwrap();
        assert_eq!(merged.click, Some(ClickEvent::RunCommand("/a".into())));
        assert_eq!(merged.hover, Some(HoverEvent::ShowText(s("t"))));
        assert_eq!(merged.insertion, Some("p".into()));
    }

    /// A child that says something WINS, for the field it says it about.
    #[test]
    fn a_child_event_overrides_the_parents() {
        let parent = Arc::new(ChatEvents {
            click: Some(ClickEvent::RunCommand("/a".into())),
            ..Default::default()
        });
        let child = ChatEvents {
            click: Some(ClickEvent::RunCommand("/b".into())),
            ..Default::default()
        };
        let merged = ChatEvents::apply_to(Some(&child), Some(&parent)).unwrap();
        assert_eq!(merged.click, Some(ClickEvent::RunCommand("/b".into())));
    }

    /// An empty child does not allocate — it hands back the parent's `Arc`.
    #[test]
    fn an_empty_child_reuses_the_parent() {
        let parent = Arc::new(ChatEvents {
            click: Some(ClickEvent::RunCommand("/a".into())),
            ..Default::default()
        });
        let merged = ChatEvents::apply_to(None, Some(&parent)).unwrap();
        assert!(Arc::ptr_eq(&merged, &parent));
        let empty = ChatEvents::default();
        let merged = ChatEvents::apply_to(Some(&empty), Some(&parent)).unwrap();
        assert!(Arc::ptr_eq(&merged, &parent));
    }

    // ---- the hover's show_text lines -------------------------------------

    use crate::chat_style::{ChatLine, ChatStyle};

    fn w6s(s: &str, style: ChatStyle) -> i32 {
        s.chars().count() as i32 * (6 + i32::from(style.bold))
    }

    fn hovered(action: &str, fields: &[(&str, Nbt)]) -> ChatStyle {
        let mut all = vec![("action", Nbt::String(action.into()))];
        all.extend_from_slice(fields);
        ChatStyle {
            events: Some(Arc::new(ChatEvents {
                hover: parse_hover_event(&compound(&all)),
                ..Default::default()
            })),
            ..ChatStyle::WHITE
        }
    }

    fn plain_lines(lines: &[ChatLine]) -> Vec<String> {
        lines.iter().map(crate::chat_style::plain_text).collect()
    }

    /// `Math.max(guiWidth / 2, 200)` — integer halving, floored at 200.
    #[test]
    fn the_hover_width_is_half_the_window_or_two_hundred() {
        assert_eq!(hover_text_width(854), 427);
        assert_eq!(hover_text_width(855), 427);
        assert_eq!(hover_text_width(320), 200);
        assert_eq!(hover_text_width(0), 200);
    }

    /// A `show_text` resolves through the ordinary component walk, so a
    /// coloured child keeps its colour and a `translate` would resolve.
    #[test]
    fn show_text_resolves_and_wraps() {
        let style = hovered("show_text", &[("value", s("aaa bbb"))]);
        let lines = show_text_lines(&style, None, 854, &w6s).unwrap();
        assert_eq!(plain_lines(&lines), ["aaa bbb"]);
        // 200 px at six pixels a character is 33; a longer line breaks, and
        // **without** `wrapComponents`' leading space.
        let long = "x".repeat(40);
        let style = hovered("show_text", &[("value", s(&long))]);
        let lines = show_text_lines(&style, None, 320, &w6s).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(!crate::chat_style::plain_text(&lines[1]).starts_with(' '));
    }

    /// The two Rewo does not resolve answer `None` rather than a plausible
    /// wrong line — and so does a style with no hover at all.
    #[test]
    fn the_other_two_hover_actions_resolve_to_nothing() {
        for action in ["show_item", "show_entity"] {
            let style = hovered(action, &[("id", s("minecraft:stone"))]);
            assert!(style.hover().is_some(), "{action} should still decode");
            assert!(show_text_lines(&style, None, 854, &w6s).is_none());
        }
        assert!(show_text_lines(&ChatStyle::WHITE, None, 854, &w6s).is_none());
    }

    /// RFC 2396's `scheme = ALPHA *( ALPHA | DIGIT | "+" | "-" | "." )`.
    ///
    /// Tested directly because the whitelist makes it unobservable from
    /// `parse_untrusted_uri` — see that function's note.
    #[test]
    fn the_scheme_grammar_is_rfc_2396s() {
        assert_eq!(uri_scheme("http://x"), Some("http"));
        assert_eq!(uri_scheme("HTTP://x"), Some("HTTP"));
        assert_eq!(uri_scheme("a+b-c.d:x"), Some("a+b-c.d"));
        // A scheme must START with a letter.
        assert_eq!(uri_scheme("1http://x"), None);
        assert_eq!(uri_scheme("+x:y"), None);
        // And may not contain anything else.
        assert_eq!(uri_scheme("a_b:x"), None);
        assert_eq!(uri_scheme("a/b:x"), None);
        // No colon at all is a relative reference — `getScheme()` is null.
        assert_eq!(uri_scheme("//example.com"), None);
        assert_eq!(uri_scheme("example.com"), None);
        // An empty first segment likewise.
        assert_eq!(uri_scheme(":x"), None);
    }
}
