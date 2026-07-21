//! Packet writer: builds the plaintext packet body (id + payload).

use crate::varint::{write_varint, write_varlong};

#[derive(Default)]
pub struct PacketWriter {
    pub buf: Vec<u8>,
}

impl PacketWriter {
    /// Start a packet with its (already-resolved) protocol id.
    pub fn packet(packet_id: i32) -> Self {
        let mut w = Self::default();
        w.varint(packet_id);
        w
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn i8(&mut self, v: i8) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u8(v as u8)
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn varint(&mut self, v: i32) -> &mut Self {
        write_varint(&mut self.buf, v);
        self
    }

    pub fn varlong(&mut self, v: i64) -> &mut Self {
        write_varlong(&mut self.buf, v);
        self
    }

    pub fn string(&mut self, s: &str) -> &mut Self {
        self.varint(s.len() as i32);
        self.buf.extend_from_slice(s.as_bytes());
        self
    }

    pub fn uuid(&mut self, v: u128) -> &mut Self {
        self.i64((v >> 64) as i64);
        self.i64(v as i64)
    }

    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(bytes);
        self
    }
}
