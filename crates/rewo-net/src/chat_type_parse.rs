//! The `minecraft:chat_type` registry and `boundChatType.decorate` (M127) —
//! the thing that turns a bare message into `<Steve> hi`.
//!
//! Every milestone since M78 has recorded this as decoded-and-not-acted-on,
//! and the blocker was real: the decoration lives in the registry, and the
//! registry is **datapack-driven**, so its contents *and its id order* are the
//! server's (M42's rule — the vector index **is** the protocol id, and nothing
//! here selects by name). The jar ships `data/minecraft/chat_type/*.json` as
//! part of the vanilla datapack, which is a cross-check oracle for the
//! transcription and is **not** a substitute for the wire: a server that
//! renames `chat.type.text` or reorders its registry is authoritative.
//!
//! # The seven vanilla types, from `ChatType.bootstrap`
//!
//! ```text
//! chat                        chat.type.text                       [sender, content]
//! say_command                 chat.type.announcement               [sender, content]
//! msg_command_incoming        commands.message.display.incoming    [sender, content]   gray+italic
//! msg_command_outgoing        commands.message.display.outgoing    [target, content]   gray+italic
//! team_msg_command_incoming   chat.type.team.text                  [target, sender, content]
//! team_msg_command_outgoing   chat.type.team.sent                  [target, sender, content]
//! emote_command               chat.type.emote                      [sender, content]
//! ```
//!
//! **`msg_command_outgoing` takes TARGET where every other two-parameter type
//! takes SENDER** (`ChatTypeDecoration.outgoingDirectMessage`), and the two
//! team decorations lead with TARGET as well — so **three of the seven** have
//! a non-SENDER parameter 0, and a reading that assumes parameter 0 is always
//! the sender is wrong about all three. `msg_command_outgoing` is the one that
//! is wrong *invisibly*: `MsgCommand` binds `name` to the sender and
//! `targetName` to the recipient, and `en_us` is "You whisper to %s: %s", so a
//! SENDER-first read renders "You whisper to <yourself>" — well-formed,
//! plausible, and wrong. The team ones would merely swap two visible names.
//!
//! # The parameters are STRINGS here and VarInts on the stream
//!
//! `ChatTypeDecoration.Parameter` carries two codecs:
//!
//! ```text
//! CODEC        = StringRepresentable.fromEnum(...)          // "sender" / "target" / "content"
//! STREAM_CODEC = ByteBufCodecs.idMapper(BY_ID, p -> p.id)   // 0 / 1 / 2
//! ```
//!
//! The registry arrives as NBT through `registry_data`, so it uses `CODEC` and
//! the parameters are **strings**. The *inline* `holder == 0` form inside a
//! `ChatType.Bound` uses `STREAM_CODEC` and they are **VarInts** — which is
//! what [`crate::session::read_chat_type_bound`]'s walker already reads. Two
//! encodings of one enum, one on each path; reading the registry with the
//! stream form finds no parameters at all and silently produces a decoration
//! that drops both the sender and the message.
//!
//! The stream form additionally resolves through
//! `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`, so an out-of-range id
//! is `SENDER` rather than an error. The string form is
//! `StringRepresentable.fromEnum`, where an unknown name is a codec failure —
//! see [`Decoration`] for what Rewo does instead of failing the connection.
//!
//! # `style` is optional and defaults to EMPTY
//!
//! `Style.Serializer.CODEC.optionalFieldOf("style", Style.EMPTY)` — five of the
//! seven vanilla types carry no `style` key at all, and that is not a
//! truncated entry. Only the two `/msg` decorations have one, and it is
//! `Style.EMPTY.withColor(GRAY).withItalic(true)`.
//!
//! # Why the decoration builds a component rather than rendering one
//!
//! `ChatTypeDecoration.decorate` is one line:
//!
//! ```java
//! return Component.translatable(this.translationKey, parameters).withStyle(this.style);
//! ```
//!
//! so the decoration is a *component construction*, not a render. Rewo builds
//! exactly that component as NBT and hands it to the resolver M125 and M126
//! already built. That is not a convenience: the composition rule the styled
//! `/msg` line needs — a template literal takes the translatable's own style
//! while a component argument applies its own **on top**, because
//! `Component.visit` opens with `getStyle().applyTo(parentStyle)` — is already
//! implemented once in [`rewo_world::chat_style`], and writing a second path
//! here would be a second chance to get it wrong. A team-coloured sender name
//! keeping its colour inside a grey `/msg` line falls out for free.

use rewo_proto::nbt::Nbt;
use rewo_proto::reader::PacketReader;

pub const CHAT_TYPE_REGISTRY: &str = "minecraft:chat_type";

/// `ChatTypeDecoration.Parameter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parameter {
    /// `chatType.name()` — the sender's display name.
    Sender,
    /// `chatType.targetName().orElse(CommonComponents.EMPTY)`.
    ///
    /// **The fallback is an empty component, not a missing argument.** The
    /// slot still exists, so `%s` still consumes it and every later `%s` still
    /// lines up; dropping it instead shifts the message into the target's
    /// place.
    Target,
    /// The message itself.
    Content,
}

impl Parameter {
    /// `StringRepresentable.fromEnum` — the registry (NBT/JSON) encoding.
    pub fn from_serialized_name(name: &str) -> Option<Self> {
        Some(match name {
            "sender" => Parameter::Sender,
            "target" => Parameter::Target,
            "content" => Parameter::Content,
            _ => return None,
        })
    }

    /// `ByteBufCodecs.idMapper(ByIdMap.continuous(.., ZERO), ..)` — the stream
    /// encoding, where an out-of-range id resolves to `SENDER` rather than
    /// failing. Not used by the registry path; here so the two encodings are
    /// stated together rather than one of them being folded into a walker.
    pub fn from_stream_id(id: i32) -> Self {
        match id {
            1 => Parameter::Target,
            2 => Parameter::Content,
            _ => Parameter::Sender,
        }
    }
}

/// `ChatTypeDecoration` — a translation key, an ordered parameter list, and a
/// style.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoration {
    /// `chat.type.text`. Resolved against the language table by the caller,
    /// not here — this crate has no business reading the jar (the rule
    /// [`crate::enchantment_parse`] states for enchantment descriptions).
    pub translation_key: String,
    /// In order. `resolveParameters` indexes this list positionally, so the
    /// order **is** the argument order.
    pub parameters: Vec<Parameter>,
    /// The `style` compound verbatim, or `None` for
    /// `optionalFieldOf("style", Style.EMPTY)`'s default.
    ///
    /// Kept as the tag rather than resolved into a
    /// [`rewo_world::chat_style::ChatStyle`] because [`Decoration::decorate`]
    /// merges it back onto a component, and a round trip through a resolved
    /// style would lose the presence/absence distinction that
    /// `Style.applyTo` is built on — `"italic": false` under an italic parent
    /// is upright, and a `ChatStyle` cannot express "said nothing about
    /// italic".
    pub style: Option<Nbt>,
}

impl Decoration {
    /// `ChatTypeDecoration.decorate(content, chatType)`, as the component
    /// vanilla constructs:
    ///
    /// ```text
    /// { translate: <key>, with: [<resolved parameters>], <style fields> }
    /// ```
    ///
    /// The caller resolves it with
    /// [`rewo_world::chat_style::parse_component`], which is where the
    /// translation, the argument styles and the legacy `§` codes are handled.
    pub fn decorate(&self, content: &Nbt, bound: &crate::session::ChatTypeBound) -> Nbt {
        let args: Vec<Nbt> = self
            .parameters
            .iter()
            .map(|p| match p {
                Parameter::Sender => bound.name.clone(),
                // `orElse(CommonComponents.EMPTY)`. `Component.empty()` is a
                // literal of "", and a bare NBT string decodes to
                // `Component.literal`, so this is that component exactly.
                Parameter::Target => bound
                    .target_name
                    .clone()
                    .unwrap_or_else(|| Nbt::String(String::new())),
                Parameter::Content => content.clone(),
            })
            .collect();

        let mut fields: Vec<(String, Nbt)> = vec![
            ("translate".to_string(), Nbt::String(self.translation_key.clone())),
            ("with".to_string(), Nbt::List(args)),
        ];
        // `withStyle(Style patch)` is `setStyle(patch.applyTo(getStyle()))`,
        // and a fresh `Component.translatable` has `Style.EMPTY`, so the patch
        // becomes the style verbatim. Only the fields the resolver reads are
        // copied — see `chat_style::STYLE_FIELDS` for why not the compound.
        if let Some(style) = &self.style {
            for key in rewo_world::chat_style::STYLE_FIELDS {
                if let Some(v) = style.get(key) {
                    fields.push((key.to_string(), v.clone()));
                }
            }
        }
        Nbt::Compound(fields)
    }

    /// Read one decoration out of its registry compound.
    ///
    /// `None` rather than a substituted default when the shape is unusable.
    /// Vanilla would fail the codec and drop the connection; inventing
    /// `DEFAULT_CHAT_DECORATION` instead would render every message of that
    /// type as `<name> text` whatever the server asked for, which is a
    /// confident wrong answer. A `None` decoration renders the content
    /// undecorated — exactly what Rewo did before this milestone, and a
    /// strictly smaller lie.
    fn read(tag: Option<&Nbt>) -> Option<Decoration> {
        let tag = tag?;
        let Some(Nbt::String(translation_key)) = tag.get("translation_key") else {
            return None;
        };
        // `Parameter.CODEC.listOf().fieldOf("parameters")` — required, and a
        // list of strings. An empty list is legal: it makes every argument
        // slot in the template unsubstituted, which is the server's business.
        let Some(Nbt::List(raw)) = tag.get("parameters") else {
            return None;
        };
        let mut parameters = Vec::with_capacity(raw.len());
        for item in raw {
            let name = item.as_str()?;
            parameters.push(Parameter::from_serialized_name(name)?);
        }
        Some(Decoration {
            translation_key: translation_key.clone(),
            parameters,
            style: tag.get("style").cloned(),
        })
    }
}

/// `ChatType` — a `chat` decoration and a `narration` decoration.
///
/// Both halves reach the client by two routes: this registry, and the inline
/// `Holder.direct` form inside a `ChatType.Bound` (see
/// [`read_chat_type_stream`]). The two use **different encodings for the
/// parameter list** and the same shape otherwise, which is why one type serves
/// both.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatType {
    /// `ChatType.chat()` — the one `Bound.decorate` uses.
    pub chat: Option<Decoration>,
    /// `ChatType.narration()`.
    ///
    /// Decoded and deliberately unused: `decorateNarration` feeds the
    /// accessibility narrator, which Rewo has no equivalent of. It is a
    /// **required** field of `ChatType.DIRECT_CODEC`, so it is read rather
    /// than skipped — and kept rather than dropped so the gap is visible in
    /// the parsed data instead of only in a comment.
    pub narration: Option<Decoration>,
}

/// One `minecraft:chat_type` entry, at the index that is its protocol id.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatTypeDef {
    /// `minecraft:chat`.
    pub id: String,
    pub ty: ChatType,
}

/// `ChatType.DIRECT_STREAM_CODEC` — two `ChatTypeDecoration.STREAM_CODEC`s.
///
/// The **stream** encoding, reached only by the inline `holder == 0` branch of
/// a `ChatType.Bound`. Its parameters are VarInts through
/// [`Parameter::from_stream_id`] where the registry's are strings, and its
/// style is one NBT tag (`Style.Serializer.TRUSTED_STREAM_CODEC` is
/// `fromCodecWithRegistriesTrusted`, i.e. a tag exactly like a `Component`).
///
/// Before M127 this was a walker that consumed the bytes and returned nothing,
/// because the sender's name is read out of the middle of the first decoration
/// otherwise. It returns them now — the bytes were always being read.
pub fn read_chat_type_stream(r: &mut PacketReader) -> rewo_proto::Result<ChatType> {
    Ok(ChatType {
        chat: Some(read_decoration_stream(r)?),
        narration: Some(read_decoration_stream(r)?),
    })
}

fn read_decoration_stream(r: &mut PacketReader) -> rewo_proto::Result<Decoration> {
    let translation_key = r.string(32767)?;
    // `Parameter.STREAM_CODEC.apply(ByteBufCodecs.list())` — an unbounded
    // `collection`, so the count is guarded by the buffer rather than a
    // constant. One VarInt per element, hence a minimum of one byte each.
    let n = r.count("chat type parameters", 1)?;
    let mut parameters = Vec::with_capacity(n);
    for _ in 0..n {
        parameters.push(Parameter::from_stream_id(r.varint()?));
    }
    let style = r.nbt()?;
    Ok(Decoration {
        translation_key,
        parameters,
        // Unlike the registry form there is no optionality on the wire: the
        // style is always present, and an unstyled decoration is an empty
        // compound rather than an absent field. Kept as `Some` so the merge
        // in `decorate` treats the two sources identically — an empty
        // compound contributes no fields, which is the same outcome.
        style: Some(style),
    })
}

/// Parse the chat-type registry's entries, in wire order.
///
/// Tolerant in the same shape as [`crate::enchantment_parse`]: a malformed
/// entry keeps its slot with no decorations, because losing the *position*
/// would misroute every chat type after it.
pub fn parse_chat_type_registry(r: &mut PacketReader, count: usize) -> Vec<ChatTypeDef> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let Ok(id) = r.identifier() else { break };
        // `Optional<NBT>` — present only because Rewo answers
        // `select_known_packs` with an empty list, i.e. "send everything".
        let data = match r.u8() {
            Ok(0) => None,
            Ok(_) => r.nbt().ok(),
            Err(_) => break,
        };
        out.push(ChatTypeDef {
            id,
            ty: ChatType {
                chat: Decoration::read(data.as_ref().and_then(|n| n.get("chat"))),
                narration: Decoration::read(data.as_ref().and_then(|n| n.get("narration"))),
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ChatTypeBound, ChatTypeRef};

    /// `<identifier> <has-data> <nbt>` per entry, as `registry_data` writes it.
    /// The NBT encoder is `dimension_parse`'s fixture writer, shared by every
    /// registry fixture so a shape mismatch shows up in all of them at once.
    fn body(entries: &[(&str, Option<Nbt>)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, nbt) in entries {
            let mut w = rewo_proto::writer::PacketWriter::default();
            w.string(name).bool(nbt.is_some());
            out.extend_from_slice(&w.buf);
            if let Some(n) = nbt {
                crate::dimension_parse::builtin::write_network_nbt(&mut out, n);
            }
        }
        out
    }

    fn compound(pairs: &[(&str, Nbt)]) -> Nbt {
        Nbt::Compound(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        )
    }

    fn s(v: &str) -> Nbt {
        Nbt::String(v.to_string())
    }

    fn list_of(names: &[&str]) -> Nbt {
        Nbt::List(names.iter().map(|n| s(n)).collect())
    }

    /// `data/minecraft/chat_type/chat.json`, verbatim.
    fn vanilla_chat() -> Nbt {
        compound(&[
            (
                "chat",
                compound(&[
                    ("parameters", list_of(&["sender", "content"])),
                    ("translation_key", s("chat.type.text")),
                ]),
            ),
            (
                "narration",
                compound(&[
                    ("parameters", list_of(&["sender", "content"])),
                    ("translation_key", s("chat.type.text.narrate")),
                ]),
            ),
        ])
    }

    /// `data/minecraft/chat_type/msg_command_outgoing.json`, verbatim.
    fn vanilla_msg_outgoing() -> Nbt {
        compound(&[
            (
                "chat",
                compound(&[
                    ("parameters", list_of(&["target", "content"])),
                    (
                        "style",
                        compound(&[("color", s("gray")), ("italic", Nbt::Byte(1))]),
                    ),
                    ("translation_key", s("commands.message.display.outgoing")),
                ]),
            ),
            (
                "narration",
                compound(&[
                    ("parameters", list_of(&["sender", "content"])),
                    ("translation_key", s("chat.type.text.narrate")),
                ]),
            ),
        ])
    }

    fn bound(name: &str, target: Option<&str>) -> ChatTypeBound {
        ChatTypeBound {
            chat_type: ChatTypeRef::Registry(0),
            name: s(name),
            target_name: target.map(s),
        }
    }

    #[test]
    fn the_vanilla_chat_entry_parses_both_decorations() {
        let b = body(&[("minecraft:chat", Some(vanilla_chat()))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "minecraft:chat");
        let chat = v[0].ty.chat.as_ref().unwrap();
        assert_eq!(chat.translation_key, "chat.type.text");
        assert_eq!(chat.parameters, vec![Parameter::Sender, Parameter::Content]);
        // `optionalFieldOf("style", Style.EMPTY)` — five of the seven vanilla
        // types carry no `style` key at all, and that is not a truncated
        // entry. Treating absence as a parse failure loses `chat` itself.
        assert_eq!(chat.style, None);
        // `narration` is required by `ChatType.DIRECT_CODEC`, so it is read
        // even though nothing renders it.
        assert_eq!(
            v[0].ty.narration.as_ref().unwrap().translation_key,
            "chat.type.text.narrate"
        );
    }

    /// The one asymmetry in the seven-entry table, and it is invisible to any
    /// reading that assumes parameter 0 is the sender.
    #[test]
    fn msg_outgoing_takes_target_where_every_other_pair_takes_sender() {
        let b = body(&[
            ("minecraft:chat", Some(vanilla_chat())),
            (
                "minecraft:msg_command_outgoing",
                Some(vanilla_msg_outgoing()),
            ),
        ]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 2);
        assert_eq!(
            v[0].ty.chat.as_ref().unwrap().parameters[0],
            Parameter::Sender
        );
        assert_eq!(
            v[1].ty.chat.as_ref().unwrap().parameters[0],
            Parameter::Target,
            "ChatTypeDecoration.outgoingDirectMessage takes TARGET first"
        );
    }

    /// The registry uses `Parameter.CODEC` (`StringRepresentable.fromEnum`) and
    /// the stream uses `Parameter.STREAM_CODEC` (`idMapper`). Reading the
    /// registry with the stream form finds no parameters and silently produces
    /// a decoration that drops both the sender and the message.
    #[test]
    fn the_registry_encodes_parameters_as_strings_not_ids() {
        let ids = compound(&[
            (
                "chat",
                compound(&[
                    ("parameters", Nbt::List(vec![Nbt::Int(0), Nbt::Int(2)])),
                    ("translation_key", s("chat.type.text")),
                ]),
            ),
            (
                "narration",
                compound(&[
                    ("parameters", list_of(&["sender"])),
                    ("translation_key", s("x")),
                ]),
            ),
        ]);
        let b = body(&[("minecraft:chat", Some(ids))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        // An id where a name belongs is not a parameter Rewo can name, and the
        // decoration is declined rather than half-built.
        assert_eq!(v[0].ty.chat, None);
        // The sibling still parses, so this is the parameter list being
        // rejected and not the entry being abandoned.
        assert!(v[0].ty.narration.is_some());
    }

    #[test]
    fn an_unknown_parameter_name_declines_rather_than_substituting_one() {
        let odd = compound(&[
            (
                "chat",
                compound(&[
                    ("parameters", list_of(&["sender", "recipient"])),
                    ("translation_key", s("chat.type.text")),
                ]),
            ),
            (
                "narration",
                compound(&[
                    ("parameters", list_of(&["sender"])),
                    ("translation_key", s("x")),
                ]),
            ),
        ]);
        let b = body(&[("minecraft:chat", Some(odd))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        assert_eq!(v[0].ty.chat, None);
    }

    /// Losing the *position* would misroute every chat type after it, so a
    /// malformed entry keeps its slot with no decorations.
    #[test]
    fn a_malformed_entry_keeps_its_slot() {
        let b = body(&[
            (
                "minecraft:broken",
                Some(compound(&[("chat", s("nonsense"))])),
            ),
            ("minecraft:chat", Some(vanilla_chat())),
        ]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 2);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].ty.chat, None);
        assert_eq!(v[0].ty.narration, None);
        assert_eq!(v[1].id, "minecraft:chat", "index 1 is still index 1");
        assert!(v[1].ty.chat.is_some());
    }

    #[test]
    fn an_entry_with_no_payload_yields_no_decoration() {
        let b = body(&[("minecraft:chat", None)]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].ty.chat, None);
    }

    // ---- decorate ----

    #[test]
    fn decorate_builds_the_component_vanilla_constructs() {
        let b = body(&[("minecraft:chat", Some(vanilla_chat()))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        let got = v[0]
            .ty
            .chat
            .as_ref()
            .unwrap()
            .decorate(&s("hi"), &bound("Steve", None));
        assert_eq!(
            got,
            compound(&[
                ("translate", s("chat.type.text")),
                ("with", Nbt::List(vec![s("Steve"), s("hi")])),
            ]),
            "Component.translatable(key, [sender, content])"
        );
    }

    #[test]
    fn decorate_merges_the_style_onto_the_translatable() {
        let b = body(&[("minecraft:msg", Some(vanilla_msg_outgoing()))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        let got = v[0]
            .ty
            .chat
            .as_ref()
            .unwrap()
            .decorate(&s("hi"), &bound("Steve", Some("Alex")));
        // `withStyle(patch)` against a fresh component's `Style.EMPTY` makes
        // the patch the style verbatim — and the arguments are TARGET then
        // CONTENT, so the sender does not appear at all.
        assert_eq!(
            got,
            compound(&[
                ("translate", s("commands.message.display.outgoing")),
                ("with", Nbt::List(vec![s("Alex"), s("hi")])),
                ("color", s("gray")),
                ("italic", Nbt::Byte(1)),
            ])
        );
    }

    /// `orElse(CommonComponents.EMPTY)`. The slot still exists, so `%s` still
    /// consumes it and every later `%s` still lines up; dropping it instead
    /// shifts the message into the target's place.
    #[test]
    fn a_missing_target_is_an_empty_component_not_a_dropped_argument() {
        let b = body(&[("minecraft:msg", Some(vanilla_msg_outgoing()))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        let got = v[0]
            .ty
            .chat
            .as_ref()
            .unwrap()
            .decorate(&s("hi"), &bound("Steve", None));
        let Some(Nbt::List(args)) = got.get("with") else {
            panic!("no arguments")
        };
        assert_eq!(args.len(), 2, "the target's slot survives its absence");
        assert_eq!(args[0], Nbt::String(String::new()));
        assert_eq!(args[1], s("hi"));
    }

    /// `Style.Serializer.CODEC` is a record codec, so a stray field in a
    /// server's style object is dropped by vanilla. Merging the compound
    /// wholesale would let a `text` field displace the translation.
    #[test]
    fn the_style_merge_copies_only_the_fields_the_resolver_reads() {
        let hostile = Decoration {
            translation_key: "chat.type.text".into(),
            parameters: vec![Parameter::Content],
            style: Some(compound(&[
                ("bold", Nbt::Byte(1)),
                ("text", s("PWNED")),
                ("extra", Nbt::List(vec![s("also this")])),
            ])),
        };
        let got = hostile.decorate(&s("hi"), &bound("Steve", None));
        assert_eq!(got.get("bold"), Some(&Nbt::Byte(1)));
        assert_eq!(got.get("text"), None);
        assert_eq!(got.get("extra"), None);
    }

    // ---- the inline stream form ----

    fn inline_bytes(decorations: &[(&str, &[i32], Nbt)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (key, params, style) in decorations {
            let mut w = rewo_proto::writer::PacketWriter::default();
            w.string(key).varint(params.len() as i32);
            for p in *params {
                w.varint(*p);
            }
            out.extend_from_slice(&w.buf);
            crate::dimension_parse::builtin::write_network_nbt(&mut out, style);
        }
        out
    }

    /// The other half of the two-encodings finding: on the stream the same
    /// enum is a VarInt.
    #[test]
    fn the_inline_stream_form_encodes_parameters_as_var_ints() {
        let b = inline_bytes(&[
            ("chat.type.text", &[0, 2], compound(&[])),
            ("chat.type.text.narrate", &[0, 2], compound(&[])),
        ]);
        let mut r = PacketReader::new(&b);
        let ty = read_chat_type_stream(&mut r).unwrap();
        assert_eq!(r.remaining(), 0, "the read must consume the whole body");
        let chat = ty.chat.as_ref().unwrap();
        assert_eq!(chat.translation_key, "chat.type.text");
        assert_eq!(chat.parameters, vec![Parameter::Sender, Parameter::Content]);
    }

    /// `ByIdMap.continuous(.., OutOfBoundsStrategy.ZERO)` — an out-of-range id
    /// is SENDER, not an error. The registry's string form has no such rule,
    /// which is why the two encodings need two functions.
    /// R3 — the inline form's style must be CARRIED, not merely read.
    ///
    /// `an_empty_inline_style_contributes_no_fields` cannot see this: an empty
    /// compound is the one fixture where `Some(empty)` and `None` agree, which
    /// is the weak-fixture shape this project keeps rediscovering. A mutation
    /// setting `style: None` in `read_decoration_stream` survives it and dies
    /// here.
    #[test]
    fn a_non_empty_inline_style_reaches_the_decorated_component() {
        let b = inline_bytes(&[
            (
                "chat.type.text",
                &[0, 2],
                compound(&[("color", s("gray")), ("italic", Nbt::Byte(1))]),
            ),
            ("chat.type.text.narrate", &[0, 2], compound(&[])),
        ]);
        let ty = read_chat_type_stream(&mut PacketReader::new(&b)).unwrap();
        let got = ty
            .chat
            .as_ref()
            .unwrap()
            .decorate(&s("hi"), &bound("Steve", None));
        assert_eq!(got.get("color"), Some(&s("gray")));
        assert_eq!(got.get("italic"), Some(&Nbt::Byte(1)));
    }

    /// R4 — `ChatTypeDecoration.CODEC` is
    /// `Parameter.CODEC.listOf().fieldOf("parameters")`, i.e. **required**.
    ///
    /// The difference is visible rather than academic: declining renders the
    /// content undecorated, where defaulting to an empty list renders
    /// `chat.type.text`'s `"<%s> %s"` with nothing substituted — a line reading
    /// `<> ` whatever the server sent.
    ///
    /// `a_malformed_entry_keeps_its_slot` cannot reach this, because
    /// `translation_key` is checked first and its fixture omits both.
    #[test]
    fn a_decoration_with_no_parameters_field_is_declined_not_defaulted() {
        let missing = compound(&[
            (
                "chat",
                compound(&[("translation_key", s("chat.type.text"))]),
            ),
            (
                "narration",
                compound(&[
                    ("parameters", list_of(&["sender"])),
                    ("translation_key", s("x")),
                ]),
            ),
        ]);
        let b = body(&[("minecraft:chat", Some(missing))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        assert_eq!(v[0].ty.chat, None);
        assert!(v[0].ty.narration.is_some(), "the sibling still parses");
    }

    /// An **empty** parameter list is legal, and distinct from a missing one:
    /// `listOf()` accepts `[]`. The server gets a template with no arguments,
    /// which is its business.
    #[test]
    fn an_empty_parameter_list_is_legal_and_not_the_same_as_a_missing_one() {
        let empty = compound(&[
            (
                "chat",
                compound(&[
                    ("parameters", Nbt::List(vec![])),
                    ("translation_key", s("chat.type.text")),
                ]),
            ),
            (
                "narration",
                compound(&[
                    ("parameters", list_of(&["sender"])),
                    ("translation_key", s("x")),
                ]),
            ),
        ]);
        let b = body(&[("minecraft:chat", Some(empty))]);
        let v = parse_chat_type_registry(&mut PacketReader::new(&b), 1);
        let chat = v[0].ty.chat.as_ref().expect("an empty list is not a missing one");
        assert!(chat.parameters.is_empty());
        let got = chat.decorate(&s("hi"), &bound("Steve", None));
        assert_eq!(got.get("with"), Some(&Nbt::List(vec![])));
    }

    /// R2 — `STYLE_FIELDS` is read by production and only three of its six
    /// entries were witnessed, so truncating it left every crate green while a
    /// decoration setting `underlined` rendered unstyled.
    ///
    /// The const's contract is "every key `resolve_style` reads", and the
    /// correspondence is what matters: if `resolve_style` ever grows a field
    /// (vanilla's `Style` carries `shadow_color`, `font`, `insertion` and the
    /// two events, none of which `ChatStyle` models), a `STYLE_FIELDS` that did
    /// not grow with it would silently under-copy.
    ///
    /// So this grades the correspondence rather than the list: each key must
    /// be one `resolve_style` actually consumes, proved by resolving a
    /// component carrying that key alone and requiring the result to DIFFER
    /// from the parent.
    #[test]
    fn every_style_field_is_one_the_resolver_reads() {
        use rewo_world::chat_style::{ChatStyle, STYLE_FIELDS};
        // A parent with every flag set and a known colour, so a field that
        // flips a flag OFF is as visible as one that flips it on.
        let parent = ChatStyle {
            color: [0.0, 0.0, 0.0],
            bold: true,
            italic: true,
            underlined: true,
            strikethrough: true,
            obfuscated: true,
            // M128 added the click/hover payload; a decoration's style never
            // carries one, and this fixture is about the six VISUAL fields
            // `STYLE_FIELDS` names.
            events: None,
        };
        // A value of the right SHAPE per key. M128 extended `STYLE_FIELDS`
        // from six to nine, and the first cut of this test assumed every
        // field was a boolean — so `click_event: 0` produced no event, the
        // style came back unchanged, and the test failed against code that
        // was right. A weak fixture of exactly the kind this project keeps
        // finding, and mine.
        for key in STYLE_FIELDS {
            let value = match key {
                "color" => s("red"),
                "insertion" => s("Steve"),
                "click_event" => Nbt::Compound(vec![
                    ("action".to_string(), s("run_command")),
                    ("command".to_string(), s("/kill")),
                ]),
                "hover_event" => Nbt::Compound(vec![
                    ("action".to_string(), s("show_text")),
                    ("value".to_string(), s("hi")),
                ]),
                // The five format flags, flipped OFF against an all-true
                // parent so an absent-versus-false confusion is as visible as
                // a dropped field.
                _ => Nbt::Byte(0),
            };
            let tag = Nbt::Compound(vec![
                ("text".to_string(), s("x")),
                (key.to_string(), value),
            ]);
            let spans =
                rewo_world::chat_style::parse_component(&tag, parent.clone(), None);
            assert_eq!(spans.len(), 1, "{key}");
            assert_ne!(
                spans[0].style(),
                parent,
                "`STYLE_FIELDS` names {key:?}, but `resolve_style` ignores it"
            );
        }
        // And the other direction: a key the resolver does NOT read must not
        // be in the list, or `decorate` would copy a field that changes
        // nothing and the list would stop describing the resolver.
        //
        // `insertion`, `click_event` and `hover_event` were on this list until
        // M128, which gave `ChatStyle` somewhere to put them — so the list
        // shrank as the model grew, which is the correspondence working.
        for absent in ["shadow_color", "font"] {
            assert!(
                !STYLE_FIELDS.contains(&absent),
                "{absent:?} is in STYLE_FIELDS but ChatStyle cannot hold it"
            );
        }
    }

    /// C3 — a negative registry id has a stated policy rather than an
    /// accidental one.
    ///
    /// `raw` is a plain VarInt, so a wire `-1` yields `Registry(-2)`, where
    /// vanilla's `byIdOrThrow(id - 1)` throws. The resolver's
    /// `usize::try_from` declines it, which is the same fallback an id past
    /// the end takes — the content, undecorated.
    #[test]
    fn a_negative_registry_id_declines_rather_than_wrapping() {
        assert!(usize::try_from(-2i32).is_err());
    }

    #[test]
    fn an_out_of_range_stream_parameter_is_sender() {
        assert_eq!(Parameter::from_stream_id(0), Parameter::Sender);
        assert_eq!(Parameter::from_stream_id(1), Parameter::Target);
        assert_eq!(Parameter::from_stream_id(2), Parameter::Content);
        assert_eq!(Parameter::from_stream_id(3), Parameter::Sender);
        assert_eq!(Parameter::from_stream_id(-1), Parameter::Sender);
        assert_eq!(Parameter::from_serialized_name("nope"), None);
    }

    /// An inline decoration's style is always present on the wire, so an
    /// unstyled one is an *empty compound* rather than an absent field — and
    /// the merge must treat that the same as the registry's absent one.
    #[test]
    fn an_empty_inline_style_contributes_no_fields() {
        let b = inline_bytes(&[
            ("chat.type.text", &[0, 2], compound(&[])),
            ("chat.type.text.narrate", &[0, 2], compound(&[])),
        ]);
        let ty = read_chat_type_stream(&mut PacketReader::new(&b)).unwrap();
        let got = ty
            .chat
            .as_ref()
            .unwrap()
            .decorate(&s("hi"), &bound("Steve", None));
        assert_eq!(
            got,
            compound(&[
                ("translate", s("chat.type.text")),
                ("with", Nbt::List(vec![s("Steve"), s("hi")])),
            ])
        );
    }
}
