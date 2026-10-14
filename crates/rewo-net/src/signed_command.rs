//! Signed commands: `SignableCommand` + `ArgumentSignatures` +
//! `ServerboundChatCommandSignedPacket`.
//!
//! `ClientPacketListener.sendCommand` parses the command against the synced
//! tree and collects every parsed argument whose type is a `SignedArgument`
//! (in 26.2 only `MessageArgument`, i.e. `minecraft:message` — `/msg`,
//! `/say`, `/me`, `/teammsg`, ...). None → the plain `chat_command` packet.
//! Otherwise it sends `chat_command_signed` with one signature per argument,
//! each over a `SignedMessageBody` of that argument's text. Without a chat
//! session the encoder yields no signatures, but the signed packet (with an
//! empty list) is still what vanilla sends.

use rewo_proto::writer::PacketWriter;

use crate::chat_sign::LastSeenUpdate;
use crate::commands::{CommandTree, NodeKind};
use crate::dispatcher::{ContextBuilder, ParseResults};

/// `MessageArgument`'s registry name — the one `SignedArgument` in 26.2.
const SIGNED_ARGUMENT_TYPE: &str = "minecraft:message";

/// `ArgumentSignatures.MAX_ARGUMENT_NAME_LENGTH`.
const MAX_ARGUMENT_NAME_LENGTH: usize = 16;

/// `ClientPacketListener.sendCommand` (`ClientPacketListener.java:2660-2661`):
/// `SignableCommand.of(this.commands.parse(command, this.suggestionsProvider))`.
///
/// The parse must resolve ids against the real block and item registries,
/// because vanilla's tree is built with `CommandBuildContext.simple(
/// registryAccess, enabledFeatures)` (`:1688`). With none, a `block_state`,
/// `block_predicate` or `item_*` argument fails to parse, the parse stops
/// there, and a `minecraft:message` argument after it is never collected: the
/// command goes out as plain `chat_command`, and a server enforcing secure
/// profiles answers `INVALID_COMMAND_SIGNATURE` instead of running it
/// (`ServerGamePacketListenerImpl.performUnsignedChatCommand`, `:1529-1533`,
/// which re-parses with its own registries and does find the argument).
pub fn signable_command(
    tree: &CommandTree,
    command: &str,
    blocks: Option<&rewo_data::blocks::Blocks>,
    items: Option<&rewo_data::items::Items>,
) -> Vec<(String, String)> {
    let units: Vec<u16> = command.encode_utf16().collect();
    let ctx = crate::dispatcher::CommandCtx {
        names: &[],
        blocks,
        items,
    };
    let parsed = crate::dispatcher::parse(tree, &units, 0, ctx);
    signable_arguments(tree, &parsed, &units)
}

/// `SignableCommand.of`: `(argument name, argument text)` for every parsed
/// signed argument, in `ArgumentVisitor` order. `command` is the string the
/// parse ran over (UTF-16 units, as the parser indexes).
pub fn signable_arguments(
    tree: &CommandTree,
    parsed: &ParseResults,
    command: &[u16],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // `ArgumentVisitor.visitArguments(..., rejectRootRedirects = true)`: the
    // root context, then each child until one restarts at the root (a
    // redirect back to the root, as `execute run` does).
    let root = &parsed.context;
    visit(tree, root, command, &mut out);
    let mut ctx = root;
    while let Some(child) = ctx.child.as_deref() {
        if child.root == root.root {
            break;
        }
        visit(tree, child, command, &mut out);
        ctx = child;
    }
    out
}

fn visit(
    tree: &CommandTree,
    ctx: &ContextBuilder,
    command: &[u16],
    out: &mut Vec<(String, String)>,
) {
    for parsed in &ctx.nodes {
        let Some(NodeKind::Argument {
            name, type_name, ..
        }) = tree.node(parsed.node).map(|n| &n.kind)
        else {
            continue;
        };
        // `values.get(argument.getName())` — the value may be absent.
        let Some((_, range)) = ctx.arguments.iter().find(|(n, _)| n == name) else {
            continue;
        };
        if type_name != SIGNED_ARGUMENT_TYPE {
            continue;
        }
        let end = range.end.min(command.len());
        let start = range.start.min(end);
        out.push((name.clone(), String::from_utf16_lossy(&command[start..end])));
    }
}

/// `ServerboundChatCommandSignedPacket.write`: command, `writeInstant`
/// (epoch millis), salt, `ArgumentSignatures` (a list of name + fixed 256-byte
/// signature), then the last-seen update.
pub fn write_signed_command(
    p: &mut PacketWriter,
    command: &str,
    timestamp_millis: i64,
    salt: i64,
    signatures: &[(String, Vec<u8>)],
    last_seen: &LastSeenUpdate,
) {
    p.string(command).i64(timestamp_millis).i64(salt);
    p.varint(signatures.len() as i32);
    for (name, sig) in signatures {
        let name: String = name.chars().take(MAX_ARGUMENT_NAME_LENGTH).collect();
        p.string(&name).raw(sig);
    }
    last_seen.write(p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{ArgumentProps, CommandNode};
    use crate::dispatcher::{parse, CommandCtx};

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// `/msg <targets> <message>`, `/say <message>`, `/kill <targets>`.
    fn tree() -> CommandTree {
        let arg = |name: &str, type_name: &str, props: ArgumentProps| NodeKind::Argument {
            name: name.into(),
            type_id: 0,
            type_name: type_name.into(),
            props,
            suggestions: None,
        };
        let n = |flags: u8, children: Vec<i32>, kind| CommandNode {
            flags,
            children,
            redirect: 0,
            kind,
        };
        let players = ArgumentProps::Entity {
            single: false,
            players_only: true,
        };
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1, 4, 6], NodeKind::Root),
                n(1, vec![2], NodeKind::Literal("msg".into())),
                n(
                    2,
                    vec![3],
                    arg("targets", "minecraft:entity", players.clone()),
                ),
                n(
                    2 | 4,
                    vec![],
                    arg("message", "minecraft:message", ArgumentProps::None),
                ),
                n(1, vec![5], NodeKind::Literal("say".into())),
                n(
                    2 | 4,
                    vec![],
                    arg("message", "minecraft:message", ArgumentProps::None),
                ),
                n(1, vec![7], NodeKind::Literal("kill".into())),
                n(2 | 4, vec![], arg("targets", "minecraft:entity", players)),
            ],
        }
    }

    fn signable(command: &str) -> Vec<(String, String)> {
        let t = tree();
        let units = u(command);
        let ctx = CommandCtx {
            names: &[],
            blocks: None,
            items: None,
        };
        let parsed = parse(&t, &units, 0, ctx);
        signable_arguments(&t, &parsed, &units)
    }

    #[test]
    fn message_argument_is_signable_with_its_exact_text() {
        assert_eq!(
            signable("msg Steve hello  world!"),
            vec![("message".into(), "hello  world!".into())]
        );
        assert_eq!(signable("say hi"), vec![("message".into(), "hi".into())]);
    }

    #[test]
    fn commands_without_message_arguments_are_not_signable() {
        assert!(signable("kill Steve").is_empty());
        assert!(signable("unknown words here").is_empty());
    }

    #[test]
    fn signed_packet_layout() {
        let last_seen = LastSeenUpdate {
            offset: 2,
            acknowledged: 0b101,
            checksum: 7,
            last_seen: vec![],
        };
        let mut p = PacketWriter::packet(0x07);
        write_signed_command(
            &mut p,
            "say hi",
            0x0102,
            -1,
            &[("message".into(), vec![0xAB; 256])],
            &last_seen,
        );
        let mut want = vec![0x07];
        want.push(6);
        want.extend_from_slice(b"say hi");
        want.extend_from_slice(&0x0102i64.to_be_bytes());
        want.extend_from_slice(&(-1i64).to_be_bytes());
        want.push(1); // one signature
        want.push(7);
        want.extend_from_slice(b"message");
        want.extend_from_slice(&[0xAB; 256]);
        want.push(2); // offset varint
        want.extend_from_slice(&[0b101, 0, 0]); // 20-bit fixed bitset
        want.push(7); // checksum
        assert_eq!(p.into_bytes(), want);
    }

    // -- registry-dependent arguments before a message ----------------------

    fn blocks() -> rewo_data::blocks::Blocks {
        rewo_data::blocks::Blocks::for_tests(
            vec!["minecraft:air".into(), "minecraft:stone".into()],
            vec![
                ("minecraft:air".to_string(), vec![]),
                ("minecraft:stone".to_string(), vec![]),
            ],
        )
    }

    fn items() -> rewo_data::items::Items {
        rewo_data::items::Items::for_tests(&["minecraft:stone", "minecraft:stick"])
    }

    /// A server-defined `/announce <block> <message>` and
    /// `/label <item> <message>` (block/item argument, then a message in the
    /// SAME context), plus vanilla's `execute if block <pos> <block> run ...`
    /// shape: the predicate node forks back to `execute`
    /// (`ExecuteCommand.addConditional`) and `run` redirects to the root.
    fn registry_tree() -> CommandTree {
        use crate::commands::FLAG_REDIRECT;
        let arg = |name: &str, type_name: &str| NodeKind::Argument {
            name: name.into(),
            type_id: 0,
            type_name: type_name.into(),
            props: ArgumentProps::None,
            suggestions: None,
        };
        let n = |flags: u8, children: Vec<i32>, redirect: i32, kind| CommandNode {
            flags,
            children,
            redirect,
            kind,
        };
        let lit = |s: &str| NodeKind::Literal(s.into());
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1, 3, 6, 9], 0, NodeKind::Root),
                n(1, vec![2], 0, lit("say")),
                n(2 | 4, vec![], 0, arg("message", "minecraft:message")),
                n(1, vec![4], 0, lit("announce")),
                n(2, vec![5], 0, arg("block", "minecraft:block_state")),
                n(2 | 4, vec![], 0, arg("message", "minecraft:message")),
                n(1, vec![7], 0, lit("label")),
                n(2, vec![8], 0, arg("item", "minecraft:item_stack")),
                n(2 | 4, vec![], 0, arg("message", "minecraft:message")),
                n(1, vec![10, 14], 0, lit("execute")),
                n(1, vec![11], 0, lit("if")),
                n(1, vec![12], 0, lit("block")),
                n(2, vec![13], 0, arg("pos", "minecraft:block_pos")),
                n(
                    2 | 4 | FLAG_REDIRECT,
                    vec![],
                    9,
                    arg("block", "minecraft:block_predicate"),
                ),
                n(1 | FLAG_REDIRECT, vec![], 0, lit("run")),
            ],
        }
    }

    /// The fix: the block and item arguments parse against the registries, so
    /// the message after them is collected and the command is signed. With an
    /// empty context (what `send_command` passed before) the parse stopped at
    /// the block/item and the message went out unsigned, which a
    /// secure-profile server rejects with `INVALID_COMMAND_SIGNATURE`.
    #[test]
    fn a_message_after_a_block_or_item_argument_is_signable_with_registries() {
        let t = registry_tree();
        let (b, i) = (blocks(), items());
        let with = |c: &str| signable_command(&t, c, Some(&b), Some(&i));
        let message = |m: &str| vec![("message".to_string(), m.to_string())];
        assert_eq!(with("announce stone hello  world"), message("hello  world"));
        assert_eq!(with("announce minecraft:stone hi"), message("hi"));
        assert_eq!(with("label minecraft:stick hi there"), message("hi there"));
        // The pre-fix context: the same commands collect nothing.
        assert!(signable_command(&t, "announce stone hello", None, None).is_empty());
        assert!(signable_command(&t, "label minecraft:stick hi", None, None).is_empty());
        // An unknown block still fails the parse, as vanilla's would.
        assert!(with("announce not_a_block hello").is_empty());
    }

    /// `ArgumentVisitor.visitArguments(.., rejectRootRedirects = true)`
    /// (`ArgumentVisitor.java:12-21`, called from `SignableCommand.of`) stops
    /// at the first context that restarts at the root. `execute ... run say`
    /// redirects to the root, so vanilla does NOT sign the `say` message there
    /// (and the server's `hasSignableArguments` agrees, so the plain packet is
    /// accepted). The block argument must parse for the walk to reach `run`
    /// at all; with registries it does, and the answer is still "unsigned".
    #[test]
    fn execute_run_say_is_not_signed_even_after_a_parsed_block_argument() {
        let t = registry_tree();
        let (b, i) = (blocks(), items());
        let units = u("execute if block ~ ~ ~ stone run say hi");
        let ctx = CommandCtx {
            names: &[],
            blocks: Some(&b),
            items: Some(&i),
        };
        let parsed = parse(&t, &units, 0, ctx);
        // The parse went all the way through: root -> `execute` fork -> root.
        let fork = parsed.context.child.as_deref().expect("forked to execute");
        assert_eq!(fork.root, 9);
        let run = fork.child.as_deref().expect("run redirected to the root");
        assert_eq!(run.root, 0);
        assert!(signable_arguments(&t, &parsed, &units).is_empty());
        assert_eq!(
            signable_command(&t, "say hi", Some(&b), Some(&i)),
            vec![("message".into(), "hi".into())]
        );
    }
}
