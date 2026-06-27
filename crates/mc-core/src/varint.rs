use bytes::{Buf, BufMut};
use thiserror::Error;

/// Error during VarInt/VarLong decoding.
#[derive(Debug, Error)]
pub enum VarIntError {
    #[error("VarInt too large: exceeds 5 bytes (max i32)")]
    TooLarge,
    #[error("VarLong too large: exceeds 10 bytes (max i64)")]
    VarLongTooLarge,
    #[error("incomplete: expected at least 1 byte")]
    Incomplete,
}

// ── VarInt (i32) ────────────────────────────────────────────

/// A Minecraft VarInt: variable-length i32 using 1–5 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct VarInt(pub i32);

impl VarInt {
    #[inline]
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// Number of bytes needed to encode this value.
    pub fn encoded_len(&self) -> usize {
        let v = self.0 as u32;
        if v < 0x80 { 1 }
        else if v < 0x4000 { 2 }
        else if v < 0x200000 { 3 }
        else if v < 0x10000000 { 4 }
        else { 5 }
    }

    /// Write into a `BufMut` buffer.
    pub fn write<B: BufMut>(&self, buf: &mut B) {
        let mut v = self.0 as u32;
        loop {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            buf.put_u8(b);
            if v == 0 {
                break;
            }
        }
    }

    /// Read from a `Buf` buffer.
    pub fn read<B: Buf>(buf: &mut B) -> Result<Self, VarIntError> {
        let mut value: u32 = 0;
        let mut shift: u32 = 0;
        loop {
            if !buf.has_remaining() {
                return Err(VarIntError::Incomplete);
            }
            let b = buf.get_u8();
            value |= ((b & 0x7F) as u32) << shift;
            if b & 0x80 == 0 {
                return Ok(Self(value as i32));
            }
            shift += 7;
            if shift >= 35 {
                return Err(VarIntError::TooLarge);
            }
        }
    }

    /// Convenience: encode to a fresh Vec<u8>.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        self.write(&mut buf);
        buf
    }

    /// Convenience: decode from a byte slice.
    /// Returns `(value, bytes_consumed)`.
    pub fn from_bytes(src: &[u8]) -> Result<(Self, usize), VarIntError> {
        let mut cursor = bytes::Bytes::copy_from_slice(src);
        let value = Self::read(&mut cursor)?;
        let consumed = src.len() - cursor.len();
        Ok((value, consumed))
    }
}

impl From<i32> for VarInt {
    fn from(v: i32) -> Self { Self(v) }
}

impl From<VarInt> for i32 {
    fn from(v: VarInt) -> i32 { v.0 }
}

// ── VarLong (i64) ────────────────────────────────────────────

/// A Minecraft VarLong: variable-length i64 using 1–10 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct VarLong(pub i64);

impl VarLong {
    #[inline]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Number of bytes needed to encode this value.
    pub fn encoded_len(&self) -> usize {
        let v = self.0 as u64;
        let mut len = 0;
        let mut val = v;
        loop {
            len += 1;
            val >>= 7;
            if val == 0 { break; }
        }
        len
    }

    /// Write into a `BufMut` buffer.
    pub fn write<B: BufMut>(&self, buf: &mut B) {
        let mut v = self.0 as u64;
        loop {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            buf.put_u8(b);
            if v == 0 {
                break;
            }
        }
    }

    /// Read from a `Buf` buffer.
    pub fn read<B: Buf>(buf: &mut B) -> Result<Self, VarIntError> {
        let mut value: u64 = 0;
        let mut shift: u64 = 0;
        loop {
            if !buf.has_remaining() {
                return Err(VarIntError::Incomplete);
            }
            let b = buf.get_u8();
            value |= ((b & 0x7F) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(Self(value as i64));
            }
            shift += 7;
            if shift >= 70 {
                return Err(VarIntError::VarLongTooLarge);
            }
        }
    }

    /// Convenience: encode to a fresh Vec<u8>.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        self.write(&mut buf);
        buf
    }

    /// Convenience: decode from a byte slice.
    /// Returns `(value, bytes_consumed)`.
    pub fn from_bytes(src: &[u8]) -> Result<(Self, usize), VarIntError> {
        let mut cursor = bytes::Bytes::copy_from_slice(src);
        let value = Self::read(&mut cursor)?;
        let consumed = src.len() - cursor.len();
        Ok((value, consumed))
    }
}

impl From<i64> for VarLong {
    fn from(v: i64) -> Self { Self(v) }
}

impl From<VarLong> for i64 {
    fn from(v: VarLong) -> i64 { v.0 }
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_zero() {
        let v = VarInt(0);
        assert_eq!(v.encoded_len(), 1);
        assert_eq!(v.to_bytes(), vec![0x00]);
        let (decoded, n) = VarInt::from_bytes(&[0x00]).unwrap();
        assert_eq!(decoded.0, 0);
        assert_eq!(n, 1);
    }

    #[test]
    fn varint_one_byte_max() {
        let v = VarInt(127);
        assert_eq!(v.encoded_len(), 1);
        assert_eq!(v.to_bytes(), vec![0x7F]);
    }

    #[test]
    fn varint_two_byte_min() {
        let v = VarInt(128);
        assert_eq!(v.encoded_len(), 2);
        assert_eq!(v.to_bytes(), vec![0x80, 0x01]);
    }

    #[test]
    fn varint_typical_packet_id() {
        // Login Start packet ID = 0x00
        let v = VarInt(0x00);
        assert_eq!(v.to_bytes(), vec![0x00]);
    }

    #[test]
    fn varint_negative_one() {
        // -1 as i32 → u32: 0xFFFFFFFF → 5 bytes
        let v = VarInt(-1);
        assert_eq!(v.encoded_len(), 5);
        let bytes = v.to_bytes();
        let (decoded, _) = VarInt::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.0, -1);
    }

    #[test]
    fn varint_max_i32() {
        let v = VarInt(i32::MAX);
        assert_eq!(v.encoded_len(), 5);
        let bytes = v.to_bytes();
        let (decoded, _) = VarInt::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.0, i32::MAX);
    }

    #[test]
    fn varint_min_i32() {
        let v = VarInt(i32::MIN);
        assert_eq!(v.encoded_len(), 5);
        let bytes = v.to_bytes();
        let (decoded, _) = VarInt::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.0, i32::MIN);
    }

    #[test]
    fn varint_protocol_version_776() {
        // Protocol version 776 → 0x88, 0x06
        let v = VarInt(776);
        assert_eq!(v.encoded_len(), 2);
        assert_eq!(v.to_bytes(), vec![0x88, 0x06]);
        let (decoded, _) = VarInt::from_bytes(&[0x88, 0x06]).unwrap();
        assert_eq!(decoded.0, 776);
    }

    #[test]
    fn varlong_zero() {
        let v = VarLong(0);
        assert_eq!(v.encoded_len(), 1);
        assert_eq!(v.to_bytes(), vec![0x00]);
    }

    #[test]
    fn varlong_max() {
        let v = VarLong(i64::MAX);
        let bytes = v.to_bytes();
        let (decoded, _) = VarLong::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.0, i64::MAX);
    }

    #[test]
    fn varlong_negative() {
        let v = VarLong(-1);
        let bytes = v.to_bytes();
        let (decoded, _) = VarLong::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.0, -1);
    }

    #[test]
    fn roundtrip_various() {
        for &val in &[0, 1, 42, 127, 128, 255, 16383, 16384, 2097151, 2097152, i32::MAX, -1, i32::MIN] {
            let v = VarInt(val);
            let bytes = v.to_bytes();
            let (decoded, n) = VarInt::from_bytes(&bytes).unwrap();
            assert_eq!(n, bytes.len(), "wrong consumed for {val}");
            assert_eq!(decoded, v, "roundtrip failed for {val}");
        }
    }

    #[test]
    fn varint_incomplete() {
        assert!(matches!(
            VarInt::from_bytes(&[]),
            Err(VarIntError::Incomplete)
        ));
        // 0x80 → MSB=1, need another byte but none follows
        assert!(matches!(
            VarInt::from_bytes(&[0x80]),
            Err(VarIntError::Incomplete)
        ));
    }

    #[test]
    fn varint_too_large() {
        // 5 bytes all with MSB set → 35 bits, exceeds u32
        assert!(matches!(
            VarInt::from_bytes(&[0x80, 0x80, 0x80, 0x80, 0x80]),
            Err(VarIntError::TooLarge)
        ));
    }
}
