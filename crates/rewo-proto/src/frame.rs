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
        // Vanilla's client installs `CompressionDecoder` with
        // `validateDecompressed = false` (`ClientHandshakePacketListenerImpl`),
        // so it rejects neither an uncompressed frame above the threshold nor
        // a compressed one below it; only the size caps below apply.
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
    ///
    /// Header and body are assembled into one buffer and handed to the stream
    /// in a single `write_all`, so an encrypted stream ciphers and sends each
    /// frame in one piece. Does not flush — the caller decides.
    pub fn write_frame(&self, w: &mut impl Write, packet: &[u8]) -> Result<()> {
        let frame = self.encode_frame(packet)?;
        w.write_all(&frame)?;
        Ok(())
    }

    /// Encode one frame: length prefix, optional compression header, body.
    pub fn encode_frame(&self, packet: &[u8]) -> Result<Vec<u8>> {
        if packet.len() > MAX_UNCOMPRESSED {
            return Err(ProtoError::Frame(format!("packet of {} bytes too large", packet.len())));
        }
        let mut out = Vec::with_capacity(packet.len() + 10);
        match self.compression_threshold {
            None => {
                write_varint(&mut out, packet.len() as i32);
                out.extend_from_slice(packet);
            }
            Some(threshold) => {
                if (packet.len() as i32) < threshold {
                    write_varint(&mut out, (packet.len() + varint_len(0)) as i32);
                    write_varint(&mut out, 0);
                    out.extend_from_slice(packet);
                } else {
                    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
                    encoder.write_all(packet)?;
                    let compressed = encoder.finish()?;
                    let data_len = packet.len() as i32;
                    write_varint(&mut out, (varint_len(data_len) + compressed.len()) as i32);
                    write_varint(&mut out, data_len);
                    out.extend_from_slice(&compressed);
                }
            }
        }
        if out.len() > MAX_FRAME + 3 {
            return Err(ProtoError::Frame(format!("frame of {} bytes too large", out.len())));
        }
        Ok(out)
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

    /// Counts `write` calls so a frame split across several can be caught.
    struct CountingWriter {
        writes: usize,
        flushes: usize,
        data: Vec<u8>,
    }

    impl Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn a_frame_is_one_write_and_no_flush() {
        for codec in [
            FrameCodec::default(),
            FrameCodec { compression_threshold: Some(4) },
            FrameCodec { compression_threshold: Some(4096) },
        ] {
            let mut w = CountingWriter { writes: 0, flushes: 0, data: Vec::new() };
            codec.write_frame(&mut w, &[7u8; 300]).unwrap();
            assert_eq!(w.writes, 1);
            assert_eq!(w.flushes, 0);
            assert_eq!(w.data, codec.encode_frame(&[7u8; 300]).unwrap());
        }
    }

    #[test]
    fn declared_uncompressed_length_is_capped_and_checked() {
        let codec = FrameCodec { compression_threshold: Some(16) };
        // data_len above the 8 MiB protocol maximum.
        let mut frame_body = Vec::new();
        write_varint(&mut frame_body, (MAX_UNCOMPRESSED + 1) as i32);
        frame_body.extend_from_slice(&[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let mut wire = Vec::new();
        write_varint(&mut wire, frame_body.len() as i32);
        wire.extend_from_slice(&frame_body);
        let (mut s, mut o) = (Vec::new(), Vec::new());
        assert!(codec
            .read_frame(&mut std::io::Cursor::new(wire), &mut s, &mut o)
            .is_err());
        // A declared length that does not match the inflated size.
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&[1u8; 40]).unwrap();
        let z = enc.finish().unwrap();
        let mut frame_body = Vec::new();
        write_varint(&mut frame_body, 41);
        frame_body.extend_from_slice(&z);
        let mut wire = Vec::new();
        write_varint(&mut wire, frame_body.len() as i32);
        wire.extend_from_slice(&frame_body);
        assert!(codec
            .read_frame(&mut std::io::Cursor::new(wire), &mut s, &mut o)
            .is_err());
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
