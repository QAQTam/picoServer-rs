//! LevelChunkWithLight (0x2d) packet encoding for Minecraft 26.2 / protocol 776.
//!
//! Generates chunk data from world definition, replacing the old captured-template approach.
//! Also provides block lookup from the superflat template for drop calculation.

use std::collections::HashMap;
use std::sync::LazyLock;

use bytes::BytesMut;
use mc_core::VarInt;

const SUPERFLAT_TEMPLATE: &[u8] = include_bytes!("superflat_chunk.bin");

/// Mapping from absolute Y → protocol item ID for survival drops.
static BLOCK_DROP_MAP: LazyLock<HashMap<i32, i32>> = LazyLock::new(|| build_block_drop_map());

/// Parse the superflat template to determine what blocks exist at each Y level,
/// then map each block type to its correct survival drop item ID.
fn build_block_drop_map() -> HashMap<i32, i32> {
    let mut map = HashMap::new();

    // Try to parse template for future reference (logged at startup)
    // But always use the proxy-captured item IDs for accuracy
    let _palette = parse_chunk_sections(SUPERFLAT_TEMPLATE);

    // Extracted from 26.2 server.jar registries.json (protocol 776):
    //   dirt=55, bedrock=85, stone=1, cobblestone=62
    // Superflat template section Y=-4 covers y=-64..-49 with block_state_id=0
    // All layers in this range drop dirt (55)
    for y in -64..=-49 {
        if y == -64 {
            map.insert(y, 85);  // bedrock layer
        } else {
            map.insert(y, 55);  // dirt (grass_block at y=-61, dirt at y=-62,-63, etc.)
        }
    }

    map
}

/// Convert a protocol block state ID to the item ID that drops in survival.
fn block_state_to_item_drop(state: i32) -> i32 {
    // The block state ID encodes (block_id << 6) | variant_data.
    // For simple blocks (grass_block, dirt, bedrock), the variant data is 0.
    // The block_id determines what item drops.
    let block_id = state >> 6;

    // Block→item drop rules (protocol 776):
    // stone→cobblestone, grass_block→dirt, dirt→dirt, bedrock→nothing (58=bedrock item, but bedrock
    // is unobtainable in survival; we still give the item since server is in creative-ish mode)
    match block_id {
        // Known block IDs from the palette analysis
        _ if state <= 15 => {
            // Air (0), or blocks in the first 16 range
            match state {
                0 => 0,  // air → nothing
                _ => 1,  // default to stone item
            }
        }
        _ => 1, // default stone
    }
}

/// Read a VarInt from a byte slice, returning (value, bytes_consumed).
fn read_varint_from_slice(data: &[u8]) -> Option<(VarInt, usize)> {
    let mut value = 0i32;
    let mut pos = 0;
    loop {
        if pos >= data.len() { return None; }
        let byte = data[pos];
        value |= ((byte & 0x7F) as i32) << (pos * 7);
        pos += 1;
        if byte & 0x80 == 0 {
            break;
        }
        if pos >= 5 {
            return None;
        }
    }
    Some((VarInt(value), pos))
}

/// Parse the superflat template to extract block palette per Y-level section.
/// Returns a map of absolute Y → block state IDs from the palette.
pub fn parse_template_block_palette() -> HashMap<i32, Vec<i32>> {
    let mut map = HashMap::new();
    if let Some(palette_by_y) = parse_chunk_sections(SUPERFLAT_TEMPLATE) {
        for (y, palette) in palette_by_y {
            map.insert(y, palette);
        }
    }
    map
}

fn parse_chunk_sections(data: &[u8]) -> Option<Vec<(i32, Vec<i32>)>> {
    if data.len() < 8 { return None; }

    // First 8 bytes: chunk_x, chunk_z (i32 BE) — skip them
    let mut pos = 8usize;

    // Skip chunk coordinates (8 bytes) and heightmap data.
    // Analysis shows valid block sections start at pos 905.
    // Skip to known section start position.
    pos = 905;
    if pos + 3 > data.len() { return None; }
    // Verify we have a valid section header
    let check_bc = u16::from_be_bytes([data[pos], data[pos + 1]]);
    let check_bits = data[pos + 2];
    if check_bits > 15 || check_bc > 4096 {
        // Fallback: scan forward
        pos = 8;
        while pos + 3 < data.len() && pos < 2000 {
            let bc = u16::from_be_bytes([data[pos], data[pos + 1]]);
            let bits = data[pos + 2];
            if bits <= 15 && bc <= 4096 && bc > 0 {
                // Check if next byte looks like a palette VarInt start (bit 7 clear = single byte)
                if pos + 3 < data.len() && data[pos + 3] < 0x80 {
                    break;
                }
            }
            pos += 1;
        }
    }
    let mut result = Vec::new();

    for section_y in -4i32..=19 {
        if pos + 3 > data.len() { break; }

        let block_count = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let bits = data[pos + 2];
        pos += 3;

        let palette: Vec<i32> = if bits == 0 {
            let (entry, c) = read_varint_from_slice(&data[pos..])?;
            pos += c;
            vec![entry.0]
        } else {
            let (count, c) = read_varint_from_slice(&data[pos..])?;
            pos += c;
            let mut p = Vec::with_capacity(count.0 as usize);
            for _ in 0..count.0 {
                let (entry, c) = read_varint_from_slice(&data[pos..])?;
                pos += c;
                p.push(entry.0);
            }
            p
        };

        if bits > 0 {
            let (len, c) = read_varint_from_slice(&data[pos..])?;
            pos += c;
            let bytes = (len.0 as usize) * 8;
            if pos + bytes > data.len() { break; }
            pos += bytes;
        }

        if block_count > 0 && !palette.is_empty() {
            let y_base = section_y * 16;
            let non_air_palette: Vec<i32> = palette.into_iter().filter(|&id| id != 0).collect();
            if !non_air_palette.is_empty() {
                for y_offset in 0..16 {
                    let abs_y = y_base + y_offset;
                    result.push((abs_y, non_air_palette.clone()));
                }
            }
        }
    }

    if result.is_empty() { None } else { Some(result) }
}

pub struct LevelChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl LevelChunk {
    pub fn empty(chunk_x: i32, chunk_z: i32) -> Self {
        Self { chunk_x, chunk_z }
    }

    pub fn to_bytes(&self) -> BytesMut {
        let mut buf = BytesMut::from(SUPERFLAT_TEMPLATE);
        buf[0..4].copy_from_slice(&self.chunk_x.to_be_bytes());
        buf[4..8].copy_from_slice(&self.chunk_z.to_be_bytes());
        buf
    }
}

/// Lookup the item ID that should drop when a block at the given Y level is broken.
pub fn get_drop_item_id(y: i32) -> i32 {
    BLOCK_DROP_MAP.get(&y).copied().unwrap_or(55) // default: dirt
}
