//! Minecraft protocol: packet framing, state machine, and individual packet types.
//!
//! ## Architecture
//!
//! Each Minecraft TCP connection progresses through states:
//! `Handshake → Status | Login → Configuration → Play`
//!
//! Packet framing:
//! ```text
//! [length: VarInt][packet_id: VarInt][data: bytes...]
//! ```
//!
//! length = encoded_size(packet_id) + data.len()

pub mod packet;
pub mod state;
pub mod handshake;
pub mod status;
pub mod login;
pub mod config;

// Re-exports
pub use packet::{Packet, PacketFrame};
pub use state::State;
