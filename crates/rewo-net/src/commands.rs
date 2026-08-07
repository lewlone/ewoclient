//! `ClientboundCommandsPacket` — the Brigadier command tree (M113).
//!
//! The first of the three class-C chat packets, and the one the other two hang
//! off: `command_suggestions` is a reply to a request the client only knows to
//! make once it can parse what you have typed against this tree.
//!
//! # An unknown argument type is fatal, and it is fatal in vanilla too
//!
//! An argument node carries a registry id and then that type's **own**
//! properties, whose length only that type knows. Vanilla's reader is
//!
//! ```java
//! ArgumentTypeInfo<?, ?> argumentType = BuiltInRegistries.COMMAND_ARGUMENT_TYPE.byId(id);
//! if (argumentType == null) {
//!    return null;
//! }
//! ArgumentTypeInfo.Template<?> argument = argumentType.deserializeFromNetwork(input);
//! ```
//!
//! — it returns **without consuming the properties**, and then reads the next
//! node from a position that is now wrong. So this is the
//! `DataComponentPatch` hazard M41 records (no length prefix, so an
//! untranscribed variant cannot be skipped) with vanilla's own handling being
//! to bail rather than to recover. Rewo makes it a decode error: a tree read
//! past a desync is a confident wrong answer, and the whole point of the tree
//! is to decide what the player may type.
//!
//! **44 of the 57 types are `SingletonArgumentInfo` and carry zero bytes.**
//! The other 13 are transcribed below. That ratio is why this is tractable at
//! all, and why the fail-closed rule costs nothing in practice.
//!
//! # The flags byte is four booleans and a two-bit type
//!
//! ```java
//! MASK_TYPE = 3; FLAG_EXECUTABLE = 4; FLAG_REDIRECT = 8;
//! FLAG_CUSTOM_SUGGESTIONS = 16; FLAG_RESTRICTED = 32;
//! ```
//!
//! and the **order the body is read in is not the order the flags are
//! declared**: children come before the redirect, and the custom-suggestions
//! identifier comes *after* the argument's properties rather than beside its
//! own flag. Reading the suggestion id where the flag sits — the natural
//! place — puts it in front of the properties and desynchronises every
//! argument node that has one.
//!
//! # `FLAG_RESTRICTED` is new and easy to miss
//!
//! Bit 32 has no body of its own, so a reader that does not know about it
//! still consumes the right number of bytes — and then reports a restricted
//! node as an ordinary one. It is decoded here because the chat screen's
//! `hasAllowedInput` is what would consult it.

use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// `MASK_TYPE` — the low two bits.
pub const MASK_TYPE: u8 = 3;
pub const FLAG_EXECUTABLE: u8 = 4;
pub const FLAG_REDIRECT: u8 = 8;
pub const FLAG_CUSTOM_SUGGESTIONS: u8 = 16;
pub const FLAG_RESTRICTED: u8 = 32;

/// `StringType`, in `readEnum` ordinal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringType {
    SingleWord,
    QuotablePhrase,
    GreedyPhrase,
}

/// An argument type's properties, for the 13 that carry any.
///
/// The other 44 are `SingletonArgumentInfo` and read nothing, which is
/// [`ArgumentProps::None`] — **not** a missing case. A type Rewo does not know
/// is an error rather than a silent `None`, because the two are
/// indistinguishable on the wire and only one of them leaves the reader in the
/// right place.
#[derive(Clone, Debug, PartialEq)]
pub enum ArgumentProps {
    /// `SingletonArgumentInfo` — zero bytes.
    None,
    /// `brigadier:float` / `double` — a flags byte, then min and max when
    /// their bits are set.
    RangeF64 { min: Option<f64>, max: Option<f64> },
    /// `brigadier:integer` / `long`.
    RangeI64 { min: Option<i64>, max: Option<i64> },
    /// `brigadier:string`.
    String(StringType),
    /// `entity` — `single` and `playersOnly`.
    Entity { single: bool, players_only: bool },
    /// `score_holder`.
    ScoreHolder { multiple: bool },
    /// `time` — a **fixed** i32 minimum, with no flags byte in front of it.
    Time { min: i32 },
    /// The five `resource*` types — one registry identifier.
    Registry(String),
}

/// One node of the tree, as it comes off the wire.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandNode {
    pub flags: u8,
    pub children: Vec<i32>,
    /// `readVarInt` when `FLAG_REDIRECT` is set, else vanilla's literal `0`.
    pub redirect: i32,
    pub kind: NodeKind,
}

/// The type-specific half of a node.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Root,
    Literal(String),
    Argument {
        name: String,
        /// The `command_argument_type` registry id, kept raw so a caller can
        /// name it against the report.
        type_id: i32,
        props: ArgumentProps,
        /// `readIdentifier` when `FLAG_CUSTOM_SUGGESTIONS` is set — and read
        /// **after** the properties, not beside its flag.
        suggestions: Option<String>,
    },
}

impl CommandNode {
    pub fn is_executable(&self) -> bool {
        self.flags & FLAG_EXECUTABLE != 0
    }

    /// `FLAG_RESTRICTED` — the node exists but this player may not run it.
    pub fn is_restricted(&self) -> bool {
        self.flags & FLAG_RESTRICTED != 0
    }

    pub fn has_redirect(&self) -> bool {
        self.flags & FLAG_REDIRECT != 0
    }

    /// The literal or argument name, or `None` for the root.
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Root => None,
            NodeKind::Literal(s) => Some(s),
            NodeKind::Argument { name, .. } => Some(name),
        }
    }
}

/// The whole tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandTree {
    pub nodes: Vec<CommandNode>,
    pub root: i32,
}

impl CommandTree {
    pub fn node(&self, index: i32) -> Option<&CommandNode> {
        usize::try_from(index).ok().and_then(|i| self.nodes.get(i))
    }

    /// The root's children, which is every top-level command.
    ///
    /// Sorted by name so a caller reading them twice gets the same order —
    /// vanilla's own iteration is over a `HashMap`, so the wire order is not a
    /// contract and anything user-visible has to impose one.
    pub fn top_level(&self) -> Vec<&str> {
        let Some(root) = self.node(self.root) else {
            return Vec::new();
        };
        let mut out: Vec<&str> = root
            .children
            .iter()
            .filter_map(|&c| self.node(c))
            .filter_map(|n| n.name())
            .collect();
        out.sort_unstable();
        out
    }
}

/// `readNode`.
///
/// The order is `flags`, `children`, the optional `redirect`, then the
/// type-specific body — **children before redirect**, which is not the order
/// the flag constants are declared in.
fn read_node<'a>(
    r: &mut PacketReader<'_>,
    known: &dyn Fn(i32) -> Option<&'a str>,
) -> Result<CommandNode> {
    let flags = r.u8()?;
    let count = r.count("command node children", 1)?;
    let mut children = Vec::with_capacity(count);
    for _ in 0..count {
        children.push(r.varint()?);
    }
    let redirect = if flags & FLAG_REDIRECT != 0 {
        r.varint()?
    } else {
        // Vanilla's literal `0`, not "no redirect" — the flag is what says
        // there is one, and node 0 is a perfectly ordinary index.
        0
    };
    let kind = match flags & MASK_TYPE {
        2 => {
            let name = r.string(32767)?;
            let type_id = r.varint()?;
            let props = read_props(r, type_id, known)?;
            // AFTER the properties. Reading it beside its own flag desyncs
            // every argument node that carries one.
            let suggestions = if flags & FLAG_CUSTOM_SUGGESTIONS != 0 {
                Some(r.identifier()?)
            } else {
                None
            };
            NodeKind::Argument {
                name,
                type_id,
                props,
                suggestions,
            }
        }
        1 => NodeKind::Literal(r.string(32767)?),
        // Type 0 is the root. Type 3 is unused, and vanilla's `else` treats it
        // as a root — so it reads no body either, which is why an unknown type
        // here is not a desync and is not an error.
        _ => NodeKind::Root,
    };
    Ok(CommandNode {
        flags,
        children,
        redirect,
        kind,
    })
}

/// `ArgumentTypeInfo.deserializeFromNetwork`, dispatched on the registry id's
/// **name** rather than its number, so a renumbered registry fails loud
/// instead of reading one type's properties as another's.
///
/// **The names are namespaced as the registry stores them**, so five are
/// `brigadier:*` and the rest are `minecraft:*` — `ArgumentTypeInfos.register`
/// passes a bare string and `Identifier` fills in the default namespace. A
/// match on the bare name compiles, falls through to the singleton arm, and
/// reads zero bytes where the type has properties: a desync that shows up
/// several nodes later.
fn read_props<'a>(
    r: &mut PacketReader<'_>,
    type_id: i32,
    known: &dyn Fn(i32) -> Option<&'a str>,
) -> Result<ArgumentProps> {
    let Some(name) = known(type_id) else {
        return Err(rewo_proto::ProtoError::Frame(format!(
            "unknown command argument type id {type_id}: its properties have no \
             length prefix, so the rest of the tree cannot be read"
        )));
    };
    Ok(match name {
        "brigadier:float" => {
            let f = r.u8()?;
            ArgumentProps::RangeF64 {
                min: (f & 1 != 0).then(|| r.f32()).transpose()?.map(f64::from),
                max: (f & 2 != 0).then(|| r.f32()).transpose()?.map(f64::from),
            }
        }
        "brigadier:double" => {
            let f = r.u8()?;
            ArgumentProps::RangeF64 {
                min: (f & 1 != 0).then(|| r.f64()).transpose()?,
                max: (f & 2 != 0).then(|| r.f64()).transpose()?,
            }
        }
        "brigadier:integer" => {
            let f = r.u8()?;
            ArgumentProps::RangeI64 {
                // `readInt` — a FIXED big-endian i32, not a var-int, in a
                // protocol that is var-int nearly everywhere.
                min: (f & 1 != 0).then(|| r.i32()).transpose()?.map(i64::from),
                max: (f & 2 != 0).then(|| r.i32()).transpose()?.map(i64::from),
            }
        }
        "brigadier:long" => {
            let f = r.u8()?;
            ArgumentProps::RangeI64 {
                min: (f & 1 != 0).then(|| r.i64()).transpose()?,
                max: (f & 2 != 0).then(|| r.i64()).transpose()?,
            }
        }
        "brigadier:string" => ArgumentProps::String(match r.varint()? {
            0 => StringType::SingleWord,
            1 => StringType::QuotablePhrase,
            2 => StringType::GreedyPhrase,
            // `readEnum` indexes `getEnumConstants()`, so vanilla throws.
            // M65's strict convention, not `ByIdMap`'s forgiving one.
            other => {
                return Err(rewo_proto::ProtoError::Frame(format!(
                    "StringType ordinal {other} out of range"
                )))
            }
        }),
        "minecraft:entity" => {
            let f = r.u8()?;
            ArgumentProps::Entity {
                single: f & 1 != 0,
                players_only: f & 2 != 0,
            }
        }
        "minecraft:score_holder" => ArgumentProps::ScoreHolder {
            multiple: r.u8()? & 1 != 0,
        },
        // **No flags byte.** `TimeArgument.Info` reads a bare `readInt`, so a
        // reader that assumed the numeric-range shape here would take the
        // minimum's first byte as flags and then read three bytes too few.
        "minecraft:time" => ArgumentProps::Time { min: r.i32()? },
        "minecraft:resource_or_tag"
        | "minecraft:resource_or_tag_key"
        | "minecraft:resource"
        | "minecraft:resource_key"
        | "minecraft:resource_selector" => ArgumentProps::Registry(r.identifier()?),
        // Every other registered type is `SingletonArgumentInfo` and reads
        // nothing. Reached only for a name the table above does not list,
        // which by construction is one of the 44.
        _ => ArgumentProps::None,
    })
}

/// `ClientboundCommandsPacket`'s body: a counted list of nodes, then the root
/// index.
///
/// `known` maps a `command_argument_type` id to its registry name; the caller
/// owns the registry, because it comes from the datagen report rather than the
/// wire (M92's rule for a built-in registry).
pub fn read_commands<'a>(
    body: &[u8],
    known: &dyn Fn(i32) -> Option<&'a str>,
) -> Result<CommandTree> {
    let mut r = PacketReader::new(body);
    let count = r.count("command nodes", 2)?;
    let mut nodes = Vec::with_capacity(count);
    for _ in 0..count {
        nodes.push(read_node(&mut r, known)?);
    }
    let root = r.varint()?;
    // **The walk must land exactly.** Every argument node's properties are
    // read by length-free, type-specific code, so a single mis-sized one
    // shifts everything after it — and the symptom is not an error but a tree
    // full of plausible-looking garbage. Leftover bytes are the only cheap
    // evidence that all 13 property shapes were right, and on a real 26.2
    // server this walks 2,017 nodes.
    if r.remaining() != 0 {
        return Err(rewo_proto::ProtoError::Frame(format!(
            "command tree left {} bytes unread — an argument type's properties              were mis-sized",
            r.remaining()
        )));
    }
    Ok(CommandTree { nodes, root })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four ids these tests use, by their real registry names.
    fn known(id: i32) -> Option<&'static str> {
        Some(match id {
            1 => "brigadier:float",
            2 => "brigadier:double",
            3 => "brigadier:integer",
            4 => "brigadier:long",
            5 => "brigadier:string",
            6 => "minecraft:entity",
            40 => "minecraft:time",
            41 => "minecraft:resource",
            42 => "minecraft:score_holder",
            50 => "minecraft:block_pos", // a singleton
            _ => return None,
        })
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

    fn utf(s: &str, out: &mut Vec<u8>) {
        varint(s.len() as i32, out);
        out.extend_from_slice(s.as_bytes());
    }

    /// A one-argument tree: root -> literal -> argument, plus the trailing
    /// root index.
    fn tree_bytes(arg_flags: u8, type_id: i32, props: &[u8], suggestion: Option<&str>) -> Vec<u8> {
        let mut b = Vec::new();
        varint(3, &mut b); // three nodes
        // 0: root, one child
        b.push(0);
        varint(1, &mut b);
        varint(1, &mut b);
        // 1: literal "give", one child
        b.push(1);
        varint(1, &mut b);
        varint(2, &mut b);
        utf("give", &mut b);
        // 2: the argument
        b.push(arg_flags);
        varint(0, &mut b); // no children
        utf("target", &mut b);
        varint(type_id, &mut b);
        b.extend_from_slice(props);
        if let Some(s) = suggestion {
            utf(s, &mut b);
        }
        varint(0, &mut b); // rootIndex
        b
    }

    // ── structure ────────────────────────────────────────────────────────

    #[test]
    fn a_tree_reads_its_nodes_and_its_root() {
        let b = tree_bytes(2 | FLAG_EXECUTABLE, 6, &[3], None);
        let t = read_commands(&b, &known).unwrap();
        assert_eq!(t.nodes.len(), 3);
        assert_eq!(t.root, 0);
        assert_eq!(t.nodes[0].kind, NodeKind::Root);
        assert_eq!(t.nodes[1].kind, NodeKind::Literal("give".into()));
        assert_eq!(t.top_level(), ["give"]);
        assert!(t.nodes[2].is_executable());
        assert_eq!(
            t.nodes[2].kind,
            NodeKind::Argument {
                name: "target".into(),
                type_id: 6,
                props: ArgumentProps::Entity {
                    single: true,
                    players_only: true
                },
                suggestions: None,
            }
        );
    }

    #[test]
    fn the_body_consumes_exactly() {
        // The point of the test is the reader position: an argument type whose
        // properties are mis-sized leaves bytes behind or runs short, and
        // neither shows up in the decoded value.
        for (type_id, props) in [
            (1i32, vec![3u8, 0, 0, 0, 0, 0, 0, 0, 0]), // float, min+max
            (2, vec![3u8; 17]),                        // double, min+max
            (3, vec![1u8, 0, 0, 0, 5]),                // integer, min only
            (4, vec![2u8, 0, 0, 0, 0, 0, 0, 0, 9]),    // long, max only
            (5, vec![1u8]),                            // string, quotable
            (40, vec![0u8, 0, 0, 0]),                  // time
            (50, vec![]),                              // a singleton
        ] {
            let b = tree_bytes(2, type_id, &props, None);
            let mut r = PacketReader::new(&b);
            let n = r.count("nodes", 2).unwrap();
            for _ in 0..n {
                super::read_node(&mut r, &known).unwrap();
            }
            r.varint().unwrap();
            assert_eq!(r.remaining(), 0, "type {type_id} left bytes behind");
        }
    }

    #[test]
    fn a_singleton_argument_reads_no_properties() {
        let b = tree_bytes(2, 50, &[], None);
        let t = read_commands(&b, &known).unwrap();
        match &t.nodes[2].kind {
            NodeKind::Argument { props, .. } => assert_eq!(*props, ArgumentProps::None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_trailing_byte_is_an_error() {
        // The guard that turns "it did not error" into "it consumed exactly".
        // A mis-sized property shape shifts everything after it and produces
        // plausible garbage rather than a failure, so the reader's final
        // position is the cheap evidence that all 13 shapes were right.
        let mut b = tree_bytes(2, 50, &[], None);
        b.push(0);
        assert!(read_commands(&b, &known).is_err());
    }

    #[test]
    fn an_unknown_argument_type_is_an_error_not_a_skip() {
        // Vanilla returns null here WITHOUT consuming the properties and then
        // reads the next node from the wrong offset. There is no length
        // prefix, so recovery is impossible — M41's rule.
        let b = tree_bytes(2, 999, &[0, 0, 0, 0], None);
        assert!(read_commands(&b, &known).is_err());
    }

    // ── the flags ────────────────────────────────────────────────────────

    #[test]
    fn the_suggestion_id_comes_after_the_properties() {
        // Read beside its own flag instead, and every argument node carrying
        // one desynchronises. The properties here are `entity`'s single byte,
        // so a swapped reader would take `minecraft:ask_server`'s length byte
        // as the flags and then read the name as a property.
        let b = tree_bytes(2 | FLAG_CUSTOM_SUGGESTIONS, 6, &[1], Some("minecraft:ask_server"));
        let t = read_commands(&b, &known).unwrap();
        match &t.nodes[2].kind {
            NodeKind::Argument {
                props, suggestions, ..
            } => {
                assert_eq!(
                    *props,
                    ArgumentProps::Entity {
                        single: true,
                        players_only: false
                    }
                );
                assert_eq!(suggestions.as_deref(), Some("minecraft:ask_server"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_redirect_is_read_only_when_its_flag_is_set() {
        // And the children come BEFORE it, which is not the order the flag
        // constants are declared in.
        let mut b = Vec::new();
        varint(1, &mut b);
        b.push(FLAG_REDIRECT); // root type, with a redirect
        varint(2, &mut b); // two children…
        varint(7, &mut b);
        varint(8, &mut b);
        varint(3, &mut b); // …then the redirect
        varint(0, &mut b);
        let t = read_commands(&b, &known).unwrap();
        assert_eq!(t.nodes[0].children, [7, 8]);
        assert_eq!(t.nodes[0].redirect, 3);
        assert!(t.nodes[0].has_redirect());
    }

    #[test]
    fn no_redirect_flag_means_the_field_is_absent_not_zero_on_the_wire() {
        // `redirect` is vanilla's literal 0, and node 0 is an ordinary index —
        // so the FLAG is what distinguishes "redirects to the root" from "does
        // not redirect", and a reader keying off the value cannot tell.
        let b = tree_bytes(2, 50, &[], None);
        let t = read_commands(&b, &known).unwrap();
        assert_eq!(t.nodes[2].redirect, 0);
        assert!(!t.nodes[2].has_redirect());
    }

    #[test]
    fn restricted_carries_no_body_of_its_own() {
        // Which is why a reader that does not know bit 32 still consumes the
        // right bytes and silently reports a restricted node as ordinary.
        let plain = read_commands(&tree_bytes(2, 50, &[], None), &known).unwrap();
        let restricted =
            read_commands(&tree_bytes(2 | FLAG_RESTRICTED, 50, &[], None), &known).unwrap();
        assert!(!plain.nodes[2].is_restricted());
        assert!(restricted.nodes[2].is_restricted());
        assert_eq!(plain.nodes[2].kind, restricted.nodes[2].kind);
    }

    // ── the property shapes ──────────────────────────────────────────────

    #[test]
    fn a_numeric_range_reads_only_the_bounds_its_flags_name() {
        let none = read_commands(&tree_bytes(2, 3, &[0], None), &known).unwrap();
        let min = read_commands(&tree_bytes(2, 3, &[1, 0, 0, 0, 7], None), &known).unwrap();
        let max = read_commands(&tree_bytes(2, 3, &[2, 0, 0, 0, 9], None), &known).unwrap();
        let both =
            read_commands(&tree_bytes(2, 3, &[3, 0, 0, 0, 1, 0, 0, 0, 2], None), &known).unwrap();
        let props = |t: &CommandTree| match &t.nodes[2].kind {
            NodeKind::Argument { props, .. } => props.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(props(&none), ArgumentProps::RangeI64 { min: None, max: None });
        assert_eq!(
            props(&min),
            ArgumentProps::RangeI64 { min: Some(7), max: None }
        );
        assert_eq!(
            props(&max),
            ArgumentProps::RangeI64 { min: None, max: Some(9) }
        );
        assert_eq!(
            props(&both),
            ArgumentProps::RangeI64 { min: Some(1), max: Some(2) }
        );
    }

    #[test]
    fn time_has_no_flags_byte() {
        // `TimeArgument.Info` reads a bare `readInt`. Assuming the numeric
        // shape takes the minimum's first byte as flags and then reads three
        // bytes too few — which shows up as a desync three nodes later, not
        // here.
        let b = tree_bytes(2, 40, &[0, 0, 0, 20], None);
        let t = read_commands(&b, &known).unwrap();
        match &t.nodes[2].kind {
            NodeKind::Argument { props, .. } => {
                assert_eq!(*props, ArgumentProps::Time { min: 20 })
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_resource_argument_carries_one_registry_identifier() {
        let mut props = Vec::new();
        utf("minecraft:item", &mut props);
        let t = read_commands(&tree_bytes(2, 41, &props, None), &known).unwrap();
        match &t.nodes[2].kind {
            NodeKind::Argument { props, .. } => assert_eq!(
                *props,
                ArgumentProps::Registry("minecraft:item".into())
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_out_of_range_string_type_is_an_error() {
        // `readEnum` indexes an array, so vanilla throws — M65's strict
        // convention rather than `ByIdMap`'s forgiving one.
        assert!(read_commands(&tree_bytes(2, 5, &[3], None), &known).is_err());
        assert!(read_commands(&tree_bytes(2, 5, &[2], None), &known).is_ok());
    }

    #[test]
    fn an_entitys_two_flags_are_independent() {
        let props = |bits: u8| {
            let t = read_commands(&tree_bytes(2, 6, &[bits], None), &known).unwrap();
            match &t.nodes[2].kind {
                NodeKind::Argument { props, .. } => props.clone(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            props(0),
            ArgumentProps::Entity { single: false, players_only: false }
        );
        assert_eq!(
            props(1),
            ArgumentProps::Entity { single: true, players_only: false }
        );
        assert_eq!(
            props(2),
            ArgumentProps::Entity { single: false, players_only: true }
        );
    }

    // ── the tree's own accessors ─────────────────────────────────────────

    #[test]
    fn top_level_is_sorted_because_the_wire_order_is_not_a_contract() {
        let mut b = Vec::new();
        varint(3, &mut b);
        b.push(0);
        varint(2, &mut b);
        varint(1, &mut b);
        varint(2, &mut b);
        b.push(1);
        varint(0, &mut b);
        utf("tp", &mut b);
        b.push(1);
        varint(0, &mut b);
        utf("give", &mut b);
        varint(0, &mut b);
        let t = read_commands(&b, &known).unwrap();
        assert_eq!(t.top_level(), ["give", "tp"]);
    }

    #[test]
    fn an_out_of_range_root_index_yields_no_commands_rather_than_panicking() {
        let mut b = Vec::new();
        varint(1, &mut b);
        b.push(0);
        varint(0, &mut b);
        varint(99, &mut b); // a root index past the end
        let t = read_commands(&b, &known).unwrap();
        assert_eq!(t.root, 99);
        assert_eq!(t.node(99), None);
        assert!(t.top_level().is_empty());
        assert_eq!(t.node(-1), None);
    }
}
