//! rewo-proto — wire-primitive layer for the vanilla Minecraft protocol.
//!
//! Scope: VarInt/VarLong, primitive reads/writes over packet buffers,
//! network NBT (unnamed root, 1.20.2+ shape), and the frame codec
//! (length-prefixed packets with optional zlib compression). No sockets,
//! no packet semantics — those live in `rewo-net`.
//!
//! Ground truth for encodings is the decompiled 26.2 Mojmap jar
//! (`%APPDATA%/EwoClient/rewo/26.2/decompiled/`), per REWO_PLAN.md §11.

pub mod frame;
pub mod nbt;
pub mod reader;
pub mod varint;
pub mod writer;

use std::fmt;

#[derive(Debug)]
pub enum ProtoError {
    /// Ran off the end of a packet buffer.
    Eof { needed: usize, remaining: usize },
    VarIntTooLong,
    LengthOutOfRange { what: &'static str, len: i64, max: usize },
    InvalidUtf8,
    Nbt(String),
    Frame(String),
    Io(std::io::Error),
}

impl fmt::Display for ProtoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtoError::Eof { needed, remaining } => {
                write!(f, "unexpected end of packet: needed {needed}, had {remaining}")
            }
            ProtoError::VarIntTooLong => write!(f, "varint too long"),
            ProtoError::LengthOutOfRange { what, len, max } => {
                write!(f, "{what} length {len} out of range (max {max})")
            }
            ProtoError::InvalidUtf8 => write!(f, "invalid utf-8 in string"),
            ProtoError::Nbt(m) => write!(f, "nbt: {m}"),
            ProtoError::Frame(m) => write!(f, "frame: {m}"),
            ProtoError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for ProtoError {}

impl From<std::io::Error> for ProtoError {
    fn from(e: std::io::Error) -> Self {
        ProtoError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, ProtoError>;
