//! Player state tracking.
//!
//! Tracks position, rotation, and entity identity for each connected player.

/// A single player's state during Play phase.
#[derive(Debug, Clone)]
pub struct Player {
    pub entity_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            entity_id: 1352,
            x: 0.0,
            y: -59.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }
    }
}
