//! NBT (Named Binary Tag) parser for Minecraft world data.
//!
//! Implements the binary NBT format used by region files, level.dat,
//! player data, and other game data files.
//!
//! ## Format
//!
//! Each tag: [type:u8][name_len:u16][name:utf8][payload...]
//! TAG_End (0x00): no name, no payload — terminates compounds.
//!
//! ## Usage
//!
//! ```no_run
//! use mc_world::nbt::NbtValue;
//! let data = std::fs::read("level.dat").unwrap();
//! let (root, _) = NbtValue::read_compound(&data).unwrap();
//! ```

use bytes::{Buf, Bytes};
use thiserror::Error;

/// NBT tag type identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TagType {
    End = 0,
    Byte = 1,
    Short = 2,
    Int = 3,
    Long = 4,
    Float = 5,
    Double = 6,
    ByteArray = 7,
    String = 8,
    List = 9,
    Compound = 10,
    IntArray = 11,
    LongArray = 12,
}

impl TryFrom<u8> for TagType {
    type Error = NbtError;
    fn try_from(v: u8) -> Result<Self, NbtError> {
        match v {
            0 => Ok(Self::End),
            1 => Ok(Self::Byte),
            2 => Ok(Self::Short),
            3 => Ok(Self::Int),
            4 => Ok(Self::Long),
            5 => Ok(Self::Float),
            6 => Ok(Self::Double),
            7 => Ok(Self::ByteArray),
            8 => Ok(Self::String),
            9 => Ok(Self::List),
            10 => Ok(Self::Compound),
            11 => Ok(Self::IntArray),
            12 => Ok(Self::LongArray),
            _ => Err(NbtError::UnknownTag(v)),
        }
    }
}

/// NBT parse error.
#[derive(Debug, Error)]
pub enum NbtError {
    #[error("unknown tag type: 0x{0:02x}")]
    UnknownTag(u8),
    #[error("incomplete: expected {expected} bytes, got {got}")]
    Incomplete { expected: usize, got: usize },
    #[error("invalid UTF-8 in string tag")]
    InvalidUtf8,
    #[error("expected TAG_Compound for root, got {0:?}")]
    ExpectedCompound(TagType),
}

/// A parsed NBT value.
#[derive(Debug, Clone)]
pub enum NbtValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtValue>),
    Compound(Vec<(String, NbtValue)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl NbtValue {
    /// Read a nameless root compound (as found in level.dat and region files).
    pub fn read_compound(src: &[u8]) -> Result<(Self, usize), NbtError> {
        let bytes = Bytes::from(src.to_vec());
        Self::read_compound_from(bytes)
    }

    fn read_compound_from(mut buf: Bytes) -> Result<(Self, usize), NbtError> {
        let total = buf.len();
        let ty = buf.get_u8();
        let ty = TagType::try_from(ty)?;
        if ty != TagType::Compound {
            return Err(NbtError::ExpectedCompound(ty));
        }
        let (_name_len, _name) = read_tag_name(&mut buf)?;
        let (compound, _) = read_compound_body(&mut buf)?;
        let consumed = total - buf.len();
        Ok((NbtValue::Compound(compound), consumed))
    }

    /// Read a named tag from a buffer.
    pub fn read_named(buf: &mut Bytes) -> Result<(String, Self), NbtError> {
        let ty_byte = buf.get_u8();
        let ty = TagType::try_from(ty_byte)?;
        if ty == TagType::End {
            return Ok((String::new(), NbtValue::Byte(0)));
        }
        let (_, name) = read_tag_name(buf)?;
        let value = read_payload(buf, ty)?;
        Ok((name, value))
    }

    /// Convenience: get a string field from a compound.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        match self {
            NbtValue::Compound(entries) => {
                for (k, v) in entries {
                    if k == key {
                        return match v {
                            NbtValue::String(s) => Some(s.as_str()),
                            _ => None,
                        };
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Convenience: get an int field from a compound.
    pub fn get_int(&self, key: &str) -> Option<i32> {
        match self {
            NbtValue::Compound(entries) => {
                for (k, v) in entries {
                    if k == key {
                        return match v {
                            NbtValue::Int(i) => Some(*i),
                            _ => None,
                        };
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Convenience: get a compound field from a compound.
    pub fn get_compound(&self, key: &str) -> Option<&[(String, NbtValue)]> {
        match self {
            NbtValue::Compound(entries) => {
                for (k, v) in entries {
                    if k == key {
                        return match v {
                            NbtValue::Compound(c) => Some(c.as_slice()),
                            _ => None,
                        };
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Convenience: get a list field from a compound.
    pub fn get_list(&self, key: &str) -> Option<&[NbtValue]> {
        match self {
            NbtValue::Compound(entries) => {
                for (k, v) in entries {
                    if k == key {
                        return match v {
                            NbtValue::List(l) => Some(l.as_slice()),
                            _ => None,
                        };
                    }
                }
                None
            }
            _ => None,
        }
    }
}

fn read_tag_name(buf: &mut Bytes) -> Result<(u16, String), NbtError> {
    if buf.remaining() < 2 {
        return Err(NbtError::Incomplete { expected: 2, got: buf.remaining() });
    }
    let len = buf.get_u16() as usize;
    if buf.remaining() < len {
        return Err(NbtError::Incomplete { expected: len, got: buf.remaining() });
    }
    let name_bytes = buf.split_to(len);
    let name = String::from_utf8(name_bytes.to_vec()).map_err(|_| NbtError::InvalidUtf8)?;
    Ok((len as u16, name))
}

fn read_compound_body(buf: &mut Bytes) -> Result<(Vec<(String, NbtValue)>, bool), NbtError> {
    let mut entries = Vec::new();
    loop {
        if buf.remaining() == 0 {
            return Err(NbtError::Incomplete { expected: 1, got: 0 });
        }
        let ty_byte = buf.get_u8();
        if ty_byte == 0 {
            return Ok((entries, true)); // TAG_End
        }
        let ty = TagType::try_from(ty_byte)?;
        let (_, name) = read_tag_name(buf)?;
        let value = read_payload(buf, ty)?;
        entries.push((name, value));
    }
}

fn read_payload(buf: &mut Bytes, ty: TagType) -> Result<NbtValue, NbtError> {
    Ok(match ty {
        TagType::Byte => {
            if buf.remaining() < 1 {
                return Err(NbtError::Incomplete { expected: 1, got: buf.remaining() });
            }
            NbtValue::Byte(buf.get_i8())
        }
        TagType::Short => {
            if buf.remaining() < 2 {
                return Err(NbtError::Incomplete { expected: 2, got: buf.remaining() });
            }
            NbtValue::Short(buf.get_i16())
        }
        TagType::Int => {
            if buf.remaining() < 4 {
                return Err(NbtError::Incomplete { expected: 4, got: buf.remaining() });
            }
            NbtValue::Int(buf.get_i32())
        }
        TagType::Long => {
            if buf.remaining() < 8 {
                return Err(NbtError::Incomplete { expected: 8, got: buf.remaining() });
            }
            NbtValue::Long(buf.get_i64())
        }
        TagType::Float => {
            if buf.remaining() < 4 {
                return Err(NbtError::Incomplete { expected: 4, got: buf.remaining() });
            }
            NbtValue::Float(buf.get_f32())
        }
        TagType::Double => {
            if buf.remaining() < 8 {
                return Err(NbtError::Incomplete { expected: 8, got: buf.remaining() });
            }
            NbtValue::Double(buf.get_f64())
        }
        TagType::ByteArray => {
            if buf.remaining() < 4 {
                return Err(NbtError::Incomplete { expected: 4, got: buf.remaining() });
            }
            let len = buf.get_i32() as usize;
            if buf.remaining() < len {
                return Err(NbtError::Incomplete { expected: len, got: buf.remaining() });
            }
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(buf.get_i8());
            }
            NbtValue::ByteArray(arr)
        }
        TagType::String => {
            if buf.remaining() < 2 {
                return Err(NbtError::Incomplete { expected: 2, got: buf.remaining() });
            }
            let len = buf.get_u16() as usize;
            if buf.remaining() < len {
                return Err(NbtError::Incomplete { expected: len, got: buf.remaining() });
            }
            let bytes = buf.split_to(len);
            let s = String::from_utf8(bytes.to_vec()).map_err(|_| NbtError::InvalidUtf8)?;
            NbtValue::String(s)
        }
        TagType::List => {
            if buf.remaining() < 5 {
                return Err(NbtError::Incomplete { expected: 5, got: buf.remaining() });
            }
            let elem_type = buf.get_u8();
            let len = buf.get_i32() as usize;
            let mut list = Vec::with_capacity(len);
            if elem_type == 0 {
                // TAG_End list — empty list
            } else {
                let elem_ty = TagType::try_from(elem_type)?;
                for _ in 0..len {
                    list.push(read_payload(buf, elem_ty)?);
                }
            }
            NbtValue::List(list)
        }
        TagType::Compound => {
            let (entries, _) = read_compound_body(buf)?;
            NbtValue::Compound(entries)
        }
        TagType::IntArray => {
            if buf.remaining() < 4 {
                return Err(NbtError::Incomplete { expected: 4, got: buf.remaining() });
            }
            let len = buf.get_i32() as usize;
            if buf.remaining() < len * 4 {
                return Err(NbtError::Incomplete { expected: len * 4, got: buf.remaining() });
            }
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(buf.get_i32());
            }
            NbtValue::IntArray(arr)
        }
        TagType::LongArray => {
            if buf.remaining() < 4 {
                return Err(NbtError::Incomplete { expected: 4, got: buf.remaining() });
            }
            let len = buf.get_i32() as usize;
            if buf.remaining() < len * 8 {
                return Err(NbtError::Incomplete { expected: len * 8, got: buf.remaining() });
            }
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(buf.get_i64());
            }
            NbtValue::LongArray(arr)
        }
        TagType::End => NbtValue::Byte(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_simple_compound() {
        // A minimal NBT compound: { "foo": 42i32 }
        let data = vec![
            0x0A, // TAG_Compound
            0x00, 0x00, // name length = 0 (root)
            0x03, // TAG_Int
            0x00, 0x03, b'f', b'o', b'o', // "foo"
            0x00, 0x00, 0x00, 0x2A, // 42
            0x00, // TAG_End
        ];
        let (val, n) = NbtValue::read_compound(&data).unwrap();
        assert_eq!(n, data.len());
        if let NbtValue::Compound(entries) = &val {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].0, "foo");
            assert_eq!(entries[0].1.get_int("").unwrap_or(0), 0);
            match &entries[0].1 {
                NbtValue::Int(i) => assert_eq!(*i, 42),
                other => panic!("expected Int, got {other:?}"),
            }
        } else {
            panic!("expected Compound");
        }
    }

    #[test]
    fn roundtrip_nested() {
        // { "a": 1i8, "b": { "c": "hello" } }
        let data = vec![
            0x0A, 0x00, 0x00, // Compound ""
            0x01, 0x00, 0x01, b'a', 0x01, // Byte "a" = 1
            0x0A, 0x00, 0x01, b'b', // Compound "b"
            0x08, 0x00, 0x01, b'c', 0x00, 0x05, b'h', b'e', b'l', b'l', b'o', // String "c" = "hello"
            0x00, // End "b"
            0x00, // End root
        ];
        let (val, _) = NbtValue::read_compound(&data).unwrap();
        let b = val.get_compound("b").unwrap();
        let c_value = b.iter().find(|(k, _)| k == "c").unwrap();
        match &c_value.1 {
            NbtValue::String(s) => assert_eq!(s, "hello"),
            other => panic!("expected String, got {other:?}"),
        }
    }
}
