//! Minecraft protocol primitives.
//!
//! ## VarInt / VarLong
//!
//! Variable-length integer encoding used throughout the Minecraft protocol.
//! Each byte contributes 7 data bits; MSB=1 means "continue reading".
//!
//! | Value range              | Bytes |
//! |--------------------------|-------|
//! | 0–127                    | 1     |
//! | 128–16383                | 2     |
//! | 16384–2097151            | 3     |
//! | 2097152–268435455        | 4     |
//! | 268435456–34359738367    | 5     |
//!
//! VarLong extends to 10 bytes for i64.

mod varint;

pub use varint::{VarInt, VarIntError, VarLong};
