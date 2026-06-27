/// Protocol state machine.
///
/// Flow: `Handshake → Status` (for ping) or `Handshake → Login → Configuration → Play`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

impl State {
    /// Convert from the next_state byte in the Handshake packet.
    pub fn from_handshake_next_state(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Status),
            2 => Some(Self::Login),
            _ => None,
        }
    }

    /// Packet ID ranges are state-dependent.
    /// Returns true if `id` is a valid packet ID in this state.
    pub fn is_valid_packet_id(&self, id: i32) -> bool {
        match self {
            Self::Handshake => id == 0x00, // only Handshake packet
            Self::Status => (0x00..=0x01).contains(&id),
            Self::Login => (0x00..=0x04).contains(&id),
            Self::Configuration => (0x00..=0x09).contains(&id), // serverbound range
            Self::Play => true, // too many to enumerate
        }
    }
}
