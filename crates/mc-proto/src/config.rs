//! Configuration phase packets (1.20.5+ / 26.x).
//!
//! Between Login and Play, the server sends registry data, enabled features,
//! and known packs to the client. Once both sides agree, they switch to Play.
//!
//! ## Packet IDs (26.2)
//!
//! Configuration-specific packets are interleaved with common packets
//! in the same VarInt ID space:
//!
//!   Common (start at 0x00): client_information, keep_alive, ping_response, etc.
//!   S→C specific: 0x03 = finish_configuration, 0x07 = registry_data,
//!                  0x0c = update_enabled_features, 0x0e = select_known_packs
//!   C→S specific: 0x03 = finish_configuration
//!
//! ## Flow
//!
//! ```text
//! Server                           Client
//!   │                                 │
//!   │── RegistryData(dimension_type) ─▶│
//!   │── RegistryData(biome) ─────────▶│
//!   │── ... (all synced registries)    │
//!   │── UpdateEnabledFeatures ────────▶│
//!   │── SelectKnownPacks ◀────────────│ (26.2)
//!   │── FinishConfiguration ─────────▶│
//!   │                                 │
//!   │        ◀══ PLAY STATE ═══▶      │
//! ```

use bytes::{Buf, BufMut, Bytes, BytesMut};
use mc_core::VarInt;

use crate::handshake;
use crate::packet::{Packet, PacketError};

// ── S→C: Registry Data ──────────────────────────────────────

/// S→C: Registry Data packet.
///
/// Carries a list of registry entries for a specific registry key.
/// The registry key identifies which registry is being synced
/// (e.g. "minecraft:dimension_type", "minecraft:worldgen/biome").
#[derive(Debug, Clone)]
pub struct ClientboundRegistryData {
    /// Namespaced registry key, e.g. "minecraft:block".
    pub registry_key: String,
    /// Raw NBT-encoded list of registry entries.
    /// Each entry contains the entry name, optional element NBT, and flags.
    pub entries: Vec<RegistryEntry>,
}

/// A single entry in a registry data packet.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// The namespaced entry ID, e.g. "minecraft:stone".
    pub id: String,
    /// Optional NBT element data (present if `has_data` is true).
    pub data: Option<Vec<u8>>,
}

impl Packet for ClientboundRegistryData {
    const ID: i32 = 0x07; // Configuration state packet ID (after common packets)

    fn write(&self, buf: &mut BytesMut) {
        handshake::write_string(buf, &self.registry_key);
        VarInt(self.entries.len() as i32).write(buf);
        for entry in &self.entries {
            handshake::write_string(buf, &entry.id);
            match &entry.data {
                Some(data) => {
                    buf.put_u8(1); // has_data = true
                    buf.extend_from_slice(data);
                }
                None => {
                    buf.put_u8(0); // has_data = false
                }
            }
        }
    }

    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let registry_key = handshake::read_string(buf)?;
        let count = VarInt::read(buf)?;
        let mut entries = Vec::with_capacity(count.0 as usize);
        for _ in 0..count.0 {
            let id = handshake::read_string(buf)?;
            if buf.remaining() < 1 {
                return Err(PacketError::Incomplete { expected: 1, got: buf.remaining() });
            }
            let has_data = buf.get_u8() != 0;
            let data = if has_data {
                // Read remaining as NBT (for now, just store raw bytes)
                // In practice, NBT compound follows
                let remaining = buf.remaining();
                let nbt_bytes = buf.split_to(remaining);
                Some(nbt_bytes.to_vec())
            } else {
                None
            };
            entries.push(RegistryEntry { id, data });
        }
        Ok(Self { registry_key, entries })
    }
}

// ── S→C: Update Enabled Features ────────────────────────────

/// S→C: Update Enabled Features (Feature Flags).
///
/// Tells the client which experimental features are enabled.
/// Features are identified by namespaced strings.
#[derive(Debug, Clone)]
pub struct ClientboundUpdateEnabledFeatures {
    pub features: Vec<String>,
}

impl Packet for ClientboundUpdateEnabledFeatures {
    const ID: i32 = 0x0c;

    fn write(&self, buf: &mut BytesMut) {
        VarInt(self.features.len() as i32).write(buf);
        for f in &self.features {
            handshake::write_string(buf, f);
        }
    }

    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let count = VarInt::read(buf)?;
        let mut features = Vec::with_capacity(count.0 as usize);
        for _ in 0..count.0 {
            features.push(handshake::read_string(buf)?);
        }
        Ok(Self { features })
    }
}

// ── S→C: Select Known Packs ─────────────────────────────────

/// S→C / C→S: Select Known Packs.
///
/// Both sides inform each other which data pack versions they know.
/// In 26.2 this is used for P2P features and version negotiation.
#[derive(Debug, Clone)]
pub struct ClientboundSelectKnownPacks {
    pub known_packs: Vec<KnownPack>,
}

/// A single known pack entry.
#[derive(Debug, Clone)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

impl Packet for ClientboundSelectKnownPacks {
    const ID: i32 = 0x0e;

    fn write(&self, buf: &mut BytesMut) {
        VarInt(self.known_packs.len() as i32).write(buf);
        for pack in &self.known_packs {
            handshake::write_string(buf, &pack.namespace);
            handshake::write_string(buf, &pack.id);
            handshake::write_string(buf, &pack.version);
        }
    }

    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        let count = VarInt::read(buf)?;
        let mut known_packs = Vec::with_capacity(count.0 as usize);
        for _ in 0..count.0 {
            known_packs.push(KnownPack {
                namespace: handshake::read_string(buf)?,
                id: handshake::read_string(buf)?,
                version: handshake::read_string(buf)?,
            });
        }
        Ok(Self { known_packs })
    }
}

// ── S→C: Finish Configuration ────────────────────────────────

/// S→C: Finish Configuration.
///
/// Empty packet — signals the client to switch to Play state.
#[derive(Debug, Clone)]
pub struct ClientboundFinishConfiguration;

impl Packet for ClientboundFinishConfiguration {
    const ID: i32 = 0x03;

    fn write(&self, _buf: &mut BytesMut) {}
    fn read(_buf: &mut Bytes) -> Result<Self, PacketError> { Ok(Self) }
}

// ── C→S: Finish Configuration ────────────────────────────────

/// C→S: Finish Configuration (acknowledgment).
#[derive(Debug, Clone)]
pub struct ServerboundFinishConfiguration;

impl Packet for ServerboundFinishConfiguration {
    const ID: i32 = 0x03;

    fn write(&self, _buf: &mut BytesMut) {}
    fn read(_buf: &mut Bytes) -> Result<Self, PacketError> { Ok(Self) }
}

// ── C→S: Client Information ──────────────────────────────────

/// C→S: Client Information (sent during Configuration).
///
/// Carries client locale, view distance, chat mode, etc.
#[derive(Debug, Clone)]
pub struct ClientInformation {
    pub locale: String,
    pub view_distance: i8,
    pub chat_mode: i32,
    pub chat_colors: bool,
    pub displayed_skin_parts: u8,
    pub main_hand: i32,
    pub enable_text_filtering: bool,
    pub allow_server_listings: bool,
}

impl Packet for ClientInformation {
    const ID: i32 = 0x00; // Serverbound common packet

    fn write(&self, buf: &mut BytesMut) {
        handshake::write_string(buf, &self.locale);
        buf.put_i8(self.view_distance);
        VarInt(self.chat_mode).write(buf);
        buf.put_u8(if self.chat_colors { 1 } else { 0 });
        buf.put_u8(self.displayed_skin_parts);
        VarInt(self.main_hand).write(buf);
        buf.put_u8(if self.enable_text_filtering { 1 } else { 0 });
        buf.put_u8(if self.allow_server_listings { 1 } else { 0 });
    }

    fn read(buf: &mut Bytes) -> Result<Self, PacketError> {
        Ok(Self {
            locale: handshake::read_string(buf)?,
            view_distance: buf.get_i8(),
            chat_mode: VarInt::read(buf)?.0,
            chat_colors: buf.get_u8() != 0,
            displayed_skin_parts: buf.get_u8(),
            main_hand: VarInt::read(buf)?.0,
            enable_text_filtering: buf.get_u8() != 0,
            allow_server_listings: buf.get_u8() != 0,
        })
    }
}
