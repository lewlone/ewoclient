//! Frame codec: `[VarInt length][body]`, where in compressed mode the body
//! is `[VarInt uncompressed_len][zlib data]` (0 = not compressed, raw
//! follows). Compression is negotiated by Login Set Compression; encryption
//! wraps the stream *outside* this layer and lands with M7.

use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::varint::{read_varint, read_varint_io, varint_len, write_varint};
use crate::{ProtoError, Result};

/// Max on-wire frame length (3-byte VarInt limit per the protocol).
pub const MAX_FRAME: usize = (1 << 21) - 1;
/// Max plaintext packet size after decompression (vanilla: 2^23).
pub const MAX_UNCOMPRESSED: usize = 1 << 23;

#[derive(Default)]
pub struct FrameCodec {
    /// `Some(threshold)` once Login (Set Compression) arrives.
    pub compression_threshold: Option<i32>,
}

impl FrameCodec {
    /// Read one frame; `out` receives the plaintext packet (id + payload).
    pub fn read_frame(
        &self,
        r: &mut impl Read,
        scratch: &mut Vec<u8>,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        let frame_len = read_varint_io(r)?;
        if frame_len < 0 || frame_len as usize > MAX_FRAME {
            return Err(ProtoError::Frame(format!("frame length {frame_len}")));
        }
        let frame_len = frame_len as usize;

        if self.compression_threshold.is_none() {
            out.resize(frame_len, 0);
            r.read_exact(out)?;
            return Ok(());
        }

        scratch.resize(frame_len, 0);
        r.read_exact(scratch)?;
        let mut pos = 0;
        let data_len = read_varint(scratch, &mut pos)?;
        let body = &scratch[pos..];
        if data_len == 0 {
            out.clear();
            out.extend_from_slice(body);
            return Ok(());
        }
        if data_len < 0 || data_len as usize > MAX_UNCOMPRESSED {
            return Err(ProtoError::Frame(format!("data length {data_len}")));
        }
        out.clear();
        out.reserve(data_len as usize);
        let mut decoder = ZlibDecoder::new(body).take(data_len as u64 + 1);
        decoder.read_to_end(out)?;
        if out.len() != data_len as usize {
            return Err(ProtoError::Frame(format!(
                "decompressed {} bytes, expected {data_len}",
                out.len()
            )));
        }
        Ok(())
    }

    /// Write one frame from a plaintext packet.
    pub fn write_frame(&self, w: &mut impl Write, packet: &[u8]) -> Result<()> {
        let mut head = Vec::with_capacity(10);
        match self.compression_threshold {
            None => {
                write_varint(&mut head, packet.len() as i32);
                w.write_all(&head)?;
                w.write_all(packet)?;
            }
            Some(threshold) => {
                if (packet.len() as i32) < threshold {
                    write_varint(&mut head, (packet.len() + varint_len(0)) as i32);
                    write_varint(&mut head, 0);
                    w.write_all(&head)?;
                    w.write_all(packet)?;
                } else {
                    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                    encoder.write_all(packet)?;
                    let compressed = encoder.finish()?;
                    let data_len = packet.len() as i32;
                    write_varint(&mut head, (varint_len(data_len) + compressed.len()) as i32);
                    write_varint(&mut head, data_len);
                    w.write_all(&head)?;
                    w.write_all(&compressed)?;
                }
            }
        }
        w.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(codec: &FrameCodec, packet: &[u8]) -> Vec<u8> {
        let mut wire = Vec::new();
        codec.write_frame(&mut wire, packet).unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let mut scratch = Vec::new();
        let mut out = Vec::new();
        codec.read_frame(&mut cursor, &mut scratch, &mut out).unwrap();
        out
    }

    #[test]
    fn uncompressed_roundtrip() {
        let codec = FrameCodec::default();
        let packet = vec![0x1b, 1, 2, 3, 4, 5];
        assert_eq!(roundtrip(&codec, &packet), packet);
    }

    #[test]
    fn compressed_below_threshold() {
        let codec = FrameCodec {
            compression_threshold: Some(256),
        };
        let packet = vec![0x02; 32];
        assert_eq!(roundtrip(&codec, &packet), packet);
    }

    #[test]
    fn compressed_above_threshold() {
        let codec = FrameCodec {
            compression_threshold: Some(16),
        };
        let packet: Vec<u8> = (0..2000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(roundtrip(&codec, &packet), packet);
    }
}
