use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

use crate::packet::{Packet, PacketError};
use crate::handshake;

/// ── C→S: Status Request (0x00) ──────────────────────────────
///
/// Sent by client to request the server status JSON.
/// Has no payload.
#[derive(Debug, Clone)]
pub struct StatusRequest;

impl Packet for StatusRequest {
    const ID: i32 = 0x00;
    fn write(&self, _buf: &mut BytesMut) {} // no payload
    fn read(_buf: &mut Bytes) -> Result<Self, PacketError> { Ok(Self) }
}

/// ── S→C: Status Response (0x00) ─────────────────────────────
///
/// Sent by server with a JSON string describing server state.
#[derive(Debug, Clone)]
pub struct StatusResponse {
    pub json: String,
}

impl Packet for StatusResponse {
    const ID: i32 = 0x00;
    fn write(&self, buf: &mut BytesMut) {
        handshake::write_string(buf, &self.json);
    }
    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let json = handshake::read_string(buf)?;
        Ok(Self { json })
    }
}

/// ── C→S: Ping Request (0x01) ────────────────────────────────
///
/// Client sends a random i64; server must echo it back.
#[derive(Debug, Clone)]
pub struct PingRequest {
    pub payload: i64,
}

impl Packet for PingRequest {
    const ID: i32 = 0x01;
    fn write(&self, buf: &mut BytesMut) {
        buf.put_i64(self.payload);
    }
    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        if buf.remaining() < 8 {
            return Err(PacketError::Incomplete { expected: 8, got: buf.remaining() });
        }
        Ok(Self { payload: buf.get_i64() })
    }
}

/// ── S→C: Pong Response (0x01) ───────────────────────────────
///
/// Server echoes the ping payload back.
#[derive(Debug, Clone)]
pub struct PongResponse {
    pub payload: i64,
}

impl Packet for PongResponse {
    const ID: i32 = 0x01;
    fn write(&self, buf: &mut BytesMut) {
        buf.put_i64(self.payload);
    }
    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        if buf.remaining() < 8 {
            return Err(PacketError::Incomplete { expected: 8, got: buf.remaining() });
        }
        Ok(Self { payload: buf.get_i64() })
    }
}

// ── Server List Ping response JSON ───────────────────────────

/// The JSON structure Minecraft clients expect in a Status Response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub version: VersionInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub players: Option<PlayersInfo>,
    pub description: Description,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enforces_secure_chat: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub name: String,
    pub protocol: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayersInfo {
    pub max: i32,
    pub online: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<PlayerSample>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSample {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Description {
    pub text: String,
}

impl ServerStatus {
    /// Build a status response for our RustMC server.
    pub fn new(online: i32, max: i32, motd: &str) -> Self {
        Self {
            version: VersionInfo {
                name: "RustMC 26.2".into(),
                protocol: 776,
            },
            players: Some(PlayersInfo {
                max,
                online,
                sample: None,
            }),
            description: Description {
                text: motd.into(),
            },
            favicon: None,
            enforces_secure_chat: Some(false),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"text":"RustMC"}"#.into())
    }
}
