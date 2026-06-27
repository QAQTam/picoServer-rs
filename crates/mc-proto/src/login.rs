//! Login state packets (stubs for Phase 2).
//!
//! Full implementation will include:
//! - Login Start (C→S 0x00) ✅
//! - Disconnect (S→C 0x00) ✅
//! - Encryption Request/Response
//! - Login Success (S→C 0x02)
//! - Login Acknowledged (C→S 0x03)

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::packet::{Packet, PacketError};
use crate::handshake;

/// C→S 0x00: Login Start.
///
/// Client sends its username and (since 1.20.5) its player UUID.
#[derive(Debug, Clone)]
pub struct LoginStart {
    pub name: String,
    pub uuid: uuid::Uuid,
}

impl Packet for LoginStart {
    const ID: i32 = 0x00;
    fn write(&self, buf: &mut BytesMut) {
        handshake::write_string(buf, &self.name);
        buf.put_u128(self.uuid.as_u128());
    }
    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let name = handshake::read_string(buf)?;
        if buf.remaining() < 16 {
            return Err(PacketError::Incomplete { expected: 16, got: buf.remaining() });
        }
        let uuid = uuid::Uuid::from_u128(buf.get_u128());
        Ok(Self { name, uuid })
    }
}

/// S→C 0x00: Disconnect (during Login state).
///
/// Sent by the server to kick the client with a reason.
#[derive(Debug, Clone)]
pub struct LoginDisconnect {
    pub reason: String, // JSON text component
}

impl Packet for LoginDisconnect {
    const ID: i32 = 0x00;
    fn write(&self, buf: &mut BytesMut) {
        handshake::write_string(buf, &self.reason);
    }
    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let reason = handshake::read_string(buf)?;
        Ok(Self { reason })
    }
}
