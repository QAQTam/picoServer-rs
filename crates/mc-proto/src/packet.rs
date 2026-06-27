use bytes::{Buf, BytesMut};
use mc_core::VarInt;
use thiserror::Error;

use crate::State;

/// Errors during packet encode/decode.
#[derive(Debug, Error)]
pub enum PacketError {
    #[error("VarInt decode failed: {0}")]
    VarInt(#[from] mc_core::VarIntError),
    #[error("incomplete frame: expected {expected} bytes, got {got}")]
    Incomplete { expected: usize, got: usize },
    #[error("packet ID {id} not valid for state {state:?}")]
    InvalidPacketId { id: i32, state: State },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// A decoded packet: raw packet ID + data payload.
#[derive(Debug, Clone)]
pub struct PacketFrame {
    pub id: i32,
    pub data: bytes::Bytes,
}

/// Trait for types that can be encoded/decoded as Minecraft packets.
pub trait Packet: Sized {
    /// The packet ID for this type.
    const ID: i32;

    /// Write packet body (NOT including length prefix or packet ID) into buf.
    fn write(&self, buf: &mut BytesMut);

    /// Read packet body from buf (length prefix and packet ID already consumed).
    fn read(buf: &mut bytes::Bytes) -> Result<Self, PacketError>;
}

// ── Frame-level helpers ──────────────────────────────────────

/// Read a complete frame from a buffer.
///
/// Returns `None` if not enough data is available yet.
/// On success, returns the frame and consumes those bytes from `buf`.
pub fn try_read_frame(buf: &mut BytesMut) -> Result<Option<PacketFrame>, PacketError> {
    if buf.is_empty() {
        return Ok(None);
    }

    // Clone the buffer so we can peek at the length without consuming.
    let peek = buf.clone().freeze();
    let Ok((length, len_bytes)) = VarInt::from_bytes(&peek) else {
        return Ok(None); // incomplete VarInt
    };
    let length = length.0 as usize;
    let frame_start = len_bytes;

    // Do we have the full frame?
    if peek.len() < frame_start + length {
        return Ok(None);
    }

    // Consume length prefix
    buf.advance(frame_start);

    // Read packet ID
    let mut body = buf.split_to(length).freeze();
    let (packet_id_varint, _) = VarInt::from_bytes(&body)
        .map_err(|e| PacketError::VarInt(e))?;
    let packet_id = packet_id_varint.0;
    let id_bytes = packet_id_varint.encoded_len();
    body.advance(id_bytes);

    Ok(Some(PacketFrame {
        id: packet_id,
        data: body,
    }))
}

/// Encode a packet into a framed buffer ready for the socket.
///
/// Format: `[total_length: VarInt][packet_id: VarInt][body...]`
pub fn encode_packet(id: i32, body: &[u8]) -> BytesMut {
    let id_varint = VarInt(id);
    let id_len = id_varint.encoded_len();
    let total_len = id_len + body.len();
    let len_varint = VarInt(total_len as i32);

    let mut buf = BytesMut::with_capacity(len_varint.encoded_len() + total_len);
    len_varint.write(&mut buf);
    id_varint.write(&mut buf);
    buf.extend_from_slice(body);
    buf
}
