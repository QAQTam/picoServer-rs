//! Server state tracking — player, tick, teleport, chunk loading, and item entities.

use crate::player::Player;
use std::collections::HashSet;
use std::time::Instant;

pub const VIEW_RADIUS: i32 = 4;
pub const INVENTORY_SIZE: usize = 46; // 9 hotbar + 27 main + 4 armor + 1 offhand + 5 crafting

/// A dropped item entity on the ground.
#[derive(Debug, Clone)]
pub struct ItemEntity {
    pub entity_id: i32,
    pub item_id: i32,
    pub count: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// A single item stack in inventory.
#[derive(Debug, Clone)]
pub struct ItemStack {
    pub item_id: i32,
    pub count: i32,
}

pub struct ServerState {
    pub player: Player,
    pub player_uuid: u128,
    pub tick_count: u64,
    pub teleport_id: i32,
    pub keepalive_id: i32,
    pub start: Instant,
    pub loaded_chunks: HashSet<(i32, i32)>,
    pub last_chunk: (i32, i32),
    pub pending_block_updates: Vec<(i64, i32)>,
    pub pending_spawns: Vec<(i32, bytes::BytesMut)>,
    pub next_entity_id: i32,
    /// Tracked item entities on the ground.
    pub item_entities: Vec<ItemEntity>,
    /// Player inventory: slots 0-8 hotbar, 9-35 main, 36-39 armor, 40 offhand.
    pub inventory: [Option<ItemStack>; INVENTORY_SIZE],
    /// Container state ID for inventory sync.
    pub container_state_id: i32,
    /// Currently selected hotbar slot (0-8).
    pub held_slot: u16,
}

impl ServerState {
    pub fn new(uuid: u128) -> Self {
        let default_pos = Player::default();
        let cx = (default_pos.x / 16.0).floor() as i32;
        let cz = (default_pos.z / 16.0).floor() as i32;
        Self {
            player: default_pos,
            player_uuid: uuid,
            tick_count: 0,
            teleport_id: 0,
            keepalive_id: 0,
            start: Instant::now(),
            loaded_chunks: HashSet::new(),
            last_chunk: (cx, cz),
            pending_block_updates: Vec::new(),
            pending_spawns: Vec::new(),
            next_entity_id: 1000,
            item_entities: Vec::new(),
            inventory: std::array::from_fn(|_| None),
            container_state_id: 0,
            held_slot: 0,
        }
    }

    pub fn next_teleport_id(&mut self) -> i32 {
        self.teleport_id += 1;
        self.teleport_id
    }

    pub fn next_keepalive_id(&mut self) -> i32 {
        self.keepalive_id = self.keepalive_id.wrapping_add(1);
        self.keepalive_id
    }

    pub fn advance_tick(&mut self) {
        self.tick_count += 1;
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start.elapsed().as_secs()
    }

    /// Returns the player's current chunk position (cx, cz).
    pub fn player_chunk(&self) -> (i32, i32) {
        let cx = (self.player.x / 16.0).floor() as i32;
        let cz = (self.player.z / 16.0).floor() as i32;
        (cx, cz)
    }

    /// Returns true if the player moved to a different chunk.
    pub fn chunk_changed(&self) -> bool {
        self.player_chunk() != self.last_chunk
    }

    /// Compute the set of chunk positions within view radius of the player.
    pub fn visible_chunks(&self) -> Vec<(i32, i32)> {
        let (cx, cz) = self.player_chunk();
        let mut chunks = Vec::new();
        for dx in -VIEW_RADIUS..=VIEW_RADIUS {
            for dz in -VIEW_RADIUS..=VIEW_RADIUS {
                chunks.push((cx + dx, cz + dz));
            }
        }
        chunks
    }

    /// Return chunks that are now visible but haven't been loaded yet.
    pub fn new_visible_chunks(&self) -> Vec<(i32, i32)> {
        self.visible_chunks()
            .into_iter()
            .filter(|c| !self.loaded_chunks.contains(c))
            .collect()
    }

    /// Mark chunks as having been sent to the client.
    pub fn mark_chunks_loaded(&mut self, chunks: &[(i32, i32)]) {
        for &c in chunks {
            self.loaded_chunks.insert(c);
        }
        self.last_chunk = self.player_chunk();
    }
}
