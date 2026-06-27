use bytes::{Buf, BufMut, Bytes, BytesMut};
use mc_core::VarInt;

use crate::packet::{Packet, PacketError};
use crate::State;

/// C→S: Handshake (packet ID 0x00 in Handshake state).
///
/// This is the first packet sent by the client. It tells the server
/// which protocol version the client speaks, which host it wants to
/// connect to, which port, and what the next state should be.
#[derive(Debug, Clone)]
pub struct Handshake {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    pub next_state: State,
}

impl Packet for Handshake {
    const ID: i32 = 0x00;

    fn write(&self, buf: &mut BytesMut) {
        VarInt(self.protocol_version).write(buf);
        write_string(buf, &self.server_address);
        buf.put_u16(self.server_port);
        VarInt(self.next_state.to_handshake_value()).write(buf);
    }

    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let proto_ver = VarInt::read(buf)?;
        let addr = read_string(buf)?;
        if buf.remaining() < 2 {
            return Err(PacketError::Incomplete { expected: 2, got: buf.remaining() });
        }
        let port = buf.get_u16();
        let next_state_val = VarInt::read(buf)?;
        let next_state = State::from_handshake_next_state(next_state_val.0)
            .ok_or(PacketError::InvalidPacketId { id: next_state_val.0, state: State::Handshake })?;

        Ok(Self {
            protocol_version: proto_ver.0,
            server_address: addr,
            server_port: port,
            next_state,
        })
    }
}

impl State {
    fn to_handshake_value(self) -> i32 {
        match self {
            State::Status => 1,
            State::Login => 2,
            _ => 0,
        }
    }
}

// ── String helpers ───────────────────────────────────────────

/// Read a Minecraft string (length-prefixed with VarInt, UTF-8).
pub fn read_string(buf: &mut Bytes) -> Result<String, PacketError> {
    let len = VarInt::read(buf)?;
    let len = len.0 as usize;
    if buf.remaining() < len {
        return Err(PacketError::Incomplete { expected: len, got: buf.remaining() });
    }
    let bytes = buf.split_to(len);
    String::from_utf8(bytes.to_vec())
        .map_err(|e| PacketError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))
}

/// Write a Minecraft string (length-prefixed with VarInt, UTF-8).
pub fn write_string(buf: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    VarInt(bytes.len() as i32).write(buf);
    buf.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Buf;

    #[test]
    fn read_write_string_roundtrip() {
        let mut buf = BytesMut::new();
        write_string(&mut buf, "localhost");
        let mut bytes = buf.freeze();
        assert_eq!(bytes.remaining(), 10); // VarInt(1) + 9 bytes
        let s = read_string(&mut bytes).unwrap();
        assert_eq!(s, "localhost");
        assert_eq!(bytes.remaining(), 0); // all consumed
    }

    #[test]
    fn handshake_roundtrip() {
        let original = Handshake {
            protocol_version: 776,
            server_address: "localhost".into(),
            server_port: 25565,
            next_state: State::Status,
        };
        let mut buf = BytesMut::new();
        original.write(&mut buf);
        let mut bytes = buf.freeze();
        let decoded = Handshake::read(&mut bytes).unwrap();
        assert_eq!(decoded.protocol_version, 776);
        assert_eq!(decoded.server_address, "localhost");
        assert_eq!(decoded.server_port, 25565);
        matches!(decoded.next_state, State::Status);
    }

    #[test]
    fn handshake_known_bytes() {
        // Handshake for protocol 776, "localhost", port 25565, next_state=1 (Status)
        // VarInt(776)=0x88 0x06, string_len=VarInt(9), "localhost", port=0x63DD, VarInt(1)=0x01
        let raw: &[u8] = &[
            0x88, 0x06,           // protocol version 776
            0x09,                 // string length 9
            b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
            0x63, 0xDD,           // port 25565
            0x01,                 // next state: Status
        ];
        let mut bytes = Bytes::from_static(raw);
        let decoded = Handshake::read(&mut bytes).unwrap();
        assert_eq!(decoded.protocol_version, 776);
        assert_eq!(decoded.server_address, "localhost");
        assert_eq!(decoded.server_port, 25565);
        matches!(decoded.next_state, State::Status);
    }
}
