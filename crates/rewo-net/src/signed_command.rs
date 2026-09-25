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

/// `SignableCommand.of`: `(argument name, argument text)` for every parsed
/// signed argument, in `ArgumentVisitor` order. `command` is the string the
/// parse ran over (UTF-16 units, as the parser indexes).
pub fn signable_arguments(tree: &CommandTree, parsed: &ParseResults, command: &[u16]) -> Vec<(String, String)> {
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

fn visit(tree: &CommandTree, ctx: &ContextBuilder, command: &[u16], out: &mut Vec<(String, String)>) {
    for parsed in &ctx.nodes {
        let Some(NodeKind::Argument { name, type_name, .. }) = tree.node(parsed.node).map(|n| &n.kind) else {
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
        let n = |flags: u8, children: Vec<i32>, kind| CommandNode { flags, children, redirect: 0, kind };
        let players = ArgumentProps::Entity { single: false, players_only: true };
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1, 4, 6], NodeKind::Root),
                n(1, vec![2], NodeKind::Literal("msg".into())),
                n(2, vec![3], arg("targets", "minecraft:entity", players.clone())),
                n(2 | 4, vec![], arg("message", "minecraft:message", ArgumentProps::None)),
                n(1, vec![5], NodeKind::Literal("say".into())),
                n(2 | 4, vec![], arg("message", "minecraft:message", ArgumentProps::None)),
                n(1, vec![7], NodeKind::Literal("kill".into())),
                n(2 | 4, vec![], arg("targets", "minecraft:entity", players)),
            ],
        }
    }

    fn signable(command: &str) -> Vec<(String, String)> {
        let t = tree();
        let units = u(command);
        let ctx = CommandCtx { names: &[], blocks: None, items: None };
        let parsed = parse(&t, &units, 0, ctx);
        signable_arguments(&t, &parsed, &units)
    }

    #[test]
    fn message_argument_is_signable_with_its_exact_text() {
        assert_eq!(signable("msg Steve hello  world!"), vec![("message".into(), "hello  world!".into())]);
        assert_eq!(signable("say hi"), vec![("message".into(), "hi".into())]);
    }

    #[test]
    fn commands_without_message_arguments_are_not_signable() {
        assert!(signable("kill Steve").is_empty());
        assert!(signable("unknown words here").is_empty());
    }

    #[test]
    fn signed_packet_layout() {
        let last_seen = LastSeenUpdate { offset: 2, acknowledged: 0b101, checksum: 7, last_seen: vec![] };
        let mut p = PacketWriter::packet(0x07);
        write_signed_command(&mut p, "say hi", 0x0102, -1, &[("message".into(), vec![0xAB; 256])], &last_seen);
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
}
