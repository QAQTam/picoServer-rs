//! Chunk data parser.
//!
//! Extracts block states, biomes, block entities, and metadata from
//! the NBT structure of a Minecraft chunk (26.x format).
//!
//! ## Structure (26.x)
//!
//! ```text
//! Chunk [x, z]
//!   ├─ DataVersion: i32
//!   ├─ xPos: i32
//!   ├─ zPos: i32
//!   ├─ Status: String ("full", "minecraft:full", etc.)
//!   ├─ sections: List<Compound>
//!   │   ├─ Y: Byte (section index, 0–23 for 384-block worlds)
//!   │   ├─ block_states: Compound
//!   │   │   ├─ palette: List<Compound>
//!   │   │   │   └─ { Name: String, Properties: Compound? }
//!   │   │   └─ data: LongArray (packed indices)
//!   │   └─ biomes: Compound
//!   │       ├─ palette: List<String>
//!   │       └─ data: LongArray
//!   ├─ block_entities: List<Compound>
//!   └─ Heightmaps: Compound
//! ```

use crate::nbt::NbtValue;

/// A block state entry from the palette.
#[derive(Debug, Clone)]
pub struct BlockState {
    /// The namespaced block name, e.g. "minecraft:stone".
    pub name: String,
    /// Optional block properties, e.g. {"facing": "north", "waterlogged": "false"}.
    pub properties: Vec<(String, String)>,
}

/// A single section (16×16×16 block volume) within a chunk.
#[derive(Debug)]
pub struct ChunkSection {
    /// Section Y index (0 = bottom of world at y=-64 for overworld).
    pub y: i8,
    /// Block state palette — maps indices to block types.
    pub palette: Vec<BlockState>,
    /// Packed block indices: each entry is `bits_per_block` wide, stored in `data`.
    pub block_data: Vec<i64>,
    /// Bits per block entry (derived from palette size or data).
    pub bits_per_block: u8,
    /// Biome palette (namespaced biome IDs).
    pub biome_palette: Vec<String>,
    /// Packed biome indices.
    pub biome_data: Vec<i64>,
}

impl ChunkSection {
    /// Get the palette index at local (x, y, z) within this section.
    /// Coordinates are 0–15.
    #[inline]
    pub fn palette_index(&self, x: usize, y: usize, z: usize) -> usize {
        if self.bits_per_block == 0 || self.palette.len() <= 1 {
            return 0;
        }
        let idx = y * 256 + z * 16 + x; // YZX order
        extract_packed(&self.block_data, self.bits_per_block, idx) as usize
    }

    /// Get the block state at local (x, y, z).
    pub fn get_block(&self, x: usize, y: usize, z: usize) -> &BlockState {
        let idx = self.palette_index(x, y, z);
        self.palette.get(idx).unwrap_or(&AIR)
    }

    /// Get the block name at local (x, y, z).
    pub fn get_block_name(&self, x: usize, y: usize, z: usize) -> &str {
        &self.get_block(x, y, z).name
    }

    /// Iterate over all non-air blocks in this section.
    pub fn iter_non_air(&self) -> BlockIter<'_> {
        BlockIter {
            section: self,
            index: 0,
        }
    }

    /// Get the biome at local (x, y, z). Biomes are stored per 4×4×4 sub-chunk
    /// with XZ at y=0 of the section (biomes are 3D in 1.18+ but stored per Y level).
    pub fn get_biome(&self, x: usize, _y: usize, z: usize) -> Option<&str> {
        if self.biome_palette.is_empty() || self.biome_data.is_empty() {
            return None;
        }
        // Biomes use same packing as block states; each 4×4×4 volume stores one biome.
        // For simplicity, use the biome at the section's base (y=0).
        let biome_idx = (z / 4) * 4 + (x / 4);
        let idx = extract_packed(&self.biome_data, 4, biome_idx) as usize; // biomes use ~4 bpb typically
        self.biome_palette.get(idx).map(|s| s.as_str())
    }

    /// Total blocks in this section (16³ = 4096).
    pub const BLOCKS_PER_SECTION: usize = 4096;
}

/// Iterator over non-air blocks in a section.
pub struct BlockIter<'a> {
    section: &'a ChunkSection,
    index: usize,
}

impl<'a> Iterator for BlockIter<'a> {
    type Item = (usize, usize, usize, &'a BlockState); // (x, y, z, state)

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < ChunkSection::BLOCKS_PER_SECTION {
            let idx = self.index;
            self.index += 1;
            let x = idx & 15;
            let z = (idx >> 4) & 15;
            let y = (idx >> 8) & 15;
            let block = self.section.get_block(x, y, z);
            if block.name != "minecraft:air" {
                return Some((x, y, z, block));
            }
        }
        None
    }
}

/// Air block singleton (palette index 0 default).
static AIR: BlockState = BlockState {
    name: String::new(),
    properties: Vec::new(),
};

/// A block entity (chest, furnace, beacon, etc.).
#[derive(Debug)]
pub struct BlockEntity {
    /// Local block X within chunk (0–15).
    pub x: i32,
    /// Local block Y (absolute world Y).
    pub y: i32,
    /// Local block Z within chunk (0–15).
    pub z: i32,
    /// The entity type ID, e.g. "minecraft:chest".
    pub id: String,
    /// Full NBT data of the block entity.
    pub data: Vec<(String, NbtValue)>,
}

/// Parsed chunk data.
#[derive(Debug)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub data_version: i32,
    pub status: String,
    pub sections: Vec<ChunkSection>,
    pub block_entities: Vec<BlockEntity>,
    /// Heightmaps: MOTION_BLOCKING, WORLD_SURFACE, etc.
    /// Each is a LongArray of 37 entries (256/7→37 longs for 256 packed 9-bit values).
    pub heightmaps: Vec<(String, Vec<i64>)>,
    /// The minimum Y of this chunk's sections (world bottom).
    pub min_y: i32,
}

impl Chunk {
    /// Parse a chunk from its NBT root compound.
    pub fn from_nbt(data: &NbtValue) -> Option<Self> {
        let x = data.get_int("xPos")?;
        let z = data.get_int("zPos")?;
        let data_version = data.get_int("DataVersion").unwrap_or(0);
        let status = data.get_string("Status").unwrap_or("unknown").to_string();

        // Parse sections
        let sections: Vec<ChunkSection> = data.get_list("sections").map(|list| {
            list.iter().filter_map(|section_nbt| {
                parse_section(section_nbt)
            }).collect()
        }).unwrap_or_default();

        // Parse block entities
        let block_entities = data.get_list("block_entities").map(|list| {
            list.iter().filter_map(|be_nbt| {
                parse_block_entity(be_nbt)
            }).collect()
        }).unwrap_or_default();

        // Parse heightmaps
        let heightmaps = data.get_compound("Heightmaps")
            .map(|hm| {
                hm.iter().filter_map(|(k, v)| {
                    match v {
                        NbtValue::LongArray(arr) => Some((k.clone(), arr.clone())),
                        _ => None,
                    }
                }).collect()
            })
            .unwrap_or_default();

        // Compute min_y from sections
        let min_y = sections.iter()
            .map(|s| s.y as i32 * 16)
            .min()
            .unwrap_or(0);

        Some(Self { x, z, data_version, status, sections, block_entities, heightmaps, min_y })
    }

    /// Get a section by its Y index.
    pub fn get_section(&self, section_y: i8) -> Option<&ChunkSection> {
        self.sections.iter().find(|s| s.y == section_y)
    }

    /// Get the block name at absolute world coordinates.
    ///
    /// Returns `"minecraft:air"` if the chunk or section doesn't exist.
    pub fn get_block(&self, world_x: i32, world_y: i32, world_z: i32) -> &str {
        let local_x = (world_x & 15) as usize;
        let local_z = (world_z & 15) as usize;
        // Section Y: floor(world_y / 16)
        let section_y = (world_y >> 4) as i8;
        let local_y = (world_y & 15) as usize;
        match self.get_section(section_y) {
            Some(sec) => sec.get_block_name(local_x, local_y, local_z),
            None => "minecraft:air",
        }
    }

    /// Iterate over all non-air blocks in this chunk with absolute coordinates.
    pub fn iter_blocks(&self) -> ChunkBlockIter<'_> {
        ChunkBlockIter {
            chunk: self,
            section_idx: 0,
            block_iter: None,
        }
    }

    /// Get the top Y (inclusive) of the highest block at (chunk_local_x, chunk_local_z).
    /// Uses the MOTION_BLOCKING heightmap if available.
    /// Returns absolute world Y.
    pub fn get_top_y(&self, local_x: u8, local_z: u8) -> Option<i32> {
        let hm = self.heightmaps.iter()
            .find(|(k, _)| k == "MOTION_BLOCKING" || k == "WORLD_SURFACE")
            .or_else(|| self.heightmaps.first())?;
        let idx = local_z as usize * 16 + local_x as usize;
        // Heightmap stores absolute world Y in 1.18+ (9 bits per entry)
        Some(extract_packed(&hm.1, 9, idx) as i32)
    }
}

/// Iterator over non-air blocks in an entire chunk.
pub struct ChunkBlockIter<'a> {
    chunk: &'a Chunk,
    section_idx: usize,
    block_iter: Option<BlockIter<'a>>,
}

impl<'a> Iterator for ChunkBlockIter<'a> {
    type Item = (i32, i32, i32, &'a BlockState); // (world_x, world_y, world_z, state)

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(ref mut iter) = self.block_iter {
                if let Some((lx, ly, lz, state)) = iter.next() {
                    let world_x = self.chunk.x * 16 + lx as i32;
                    let world_y = self.chunk.sections[self.section_idx].y as i32 * 16 + ly as i32;
                    let world_z = self.chunk.z * 16 + lz as i32;
                    return Some((world_x, world_y, world_z, state));
                }
            }
            // Move to next section
            if self.section_idx >= self.chunk.sections.len() {
                return None;
            }
            self.block_iter = Some(self.chunk.sections[self.section_idx].iter_non_air());
            self.section_idx += 1;
        }
    }
}

fn parse_section(nbt: &NbtValue) -> Option<ChunkSection> {
    let y = match nbt {
        NbtValue::Compound(entries) => {
            entries.iter().find(|(k, _)| k == "Y")
                .and_then(|(_, v)| match v { NbtValue::Byte(b) => Some(*b), _ => None })?
        }
        _ => return None,
    };

    // Parse block_states
    let (palette, block_data, bits_per_block) = nbt.get_compound("block_states")
        .and_then(|bs| {
            let palette = parse_palette(bs)?;
            let data = bs.iter().find(|(k, _)| k == "data")
                .and_then(|(_, v)| match v { NbtValue::LongArray(arr) => Some(arr.clone()), _ => None })
                .unwrap_or_default();
            let bpb = calculate_bits_per_block(palette.len());
            Some((palette, data, bpb))
        })
        .unwrap_or_default();

    // Parse biomes
    let (biome_palette, biome_data) = nbt.get_compound("biomes")
        .map(|bio| {
            let bp = bio.iter().find(|(k, _)| k == "palette")
                .and_then(|(_, v)| match v {
                    NbtValue::List(list) => Some(
                        list.iter().filter_map(|e| match e {
                            NbtValue::String(s) => Some(s.clone()),
                            _ => None,
                        }).collect()
                    ),
                    _ => None,
                }).unwrap_or_default();
            let bd = bio.iter().find(|(k, _)| k == "data")
                .and_then(|(_, v)| match v { NbtValue::LongArray(arr) => Some(arr.clone()), _ => None })
                .unwrap_or_default();
            (bp, bd)
        })
        .unwrap_or_default();

    Some(ChunkSection { y, palette, block_data, bits_per_block, biome_palette, biome_data })
}

fn parse_palette(block_states: &[(String, NbtValue)]) -> Option<Vec<BlockState>> {
    block_states.iter().find(|(k, _)| k == "palette")
        .and_then(|(_, v)| match v {
            NbtValue::List(list) => Some(
                list.iter().map(|entry| {
                    let name = entry.get_string("Name").unwrap_or("minecraft:air").to_string();
                    let properties = entry.get_compound("Properties")
                        .map(|props| {
                            props.iter().map(|(pk, pv)| {
                                let val = match pv {
                                    NbtValue::String(s) => s.clone(),
                                    NbtValue::Int(i) => i.to_string(),
                                    NbtValue::Byte(b) => b.to_string(),
                                    _ => "?".to_string(),
                                };
                                (pk.clone(), val)
                            }).collect()
                        })
                        .unwrap_or_default();
                    BlockState { name, properties }
                }).collect()
            ),
            _ => None,
        })
}

fn parse_block_entity(nbt: &NbtValue) -> Option<BlockEntity> {
    let entries = match nbt {
        NbtValue::Compound(entries) => entries,
        _ => return None,
    };
    let x = entries.iter().find(|(k, _)| k == "x")
        .and_then(|(_, v)| match v { NbtValue::Int(i) => Some(*i), _ => None })?;
    let y = entries.iter().find(|(k, _)| k == "y")
        .and_then(|(_, v)| match v { NbtValue::Int(i) => Some(*i), _ => None })?;
    let z = entries.iter().find(|(k, _)| k == "z")
        .and_then(|(_, v)| match v { NbtValue::Int(i) => Some(*i), _ => None })?;
    let id = entries.iter().find(|(k, _)| k == "id")
        .and_then(|(_, v)| match v { NbtValue::String(s) => Some(s.clone()), _ => None })?;
    let data = entries.clone();
    Some(BlockEntity { x, y, z, id, data })
}

/// Calculate bits per block from palette size.
fn calculate_bits_per_block(palette_size: usize) -> u8 {
    if palette_size <= 1 { return 0; }
    let mut bits = 4u8;
    while (1u64 << bits) < palette_size as u64 {
        bits += 1;
    }
    bits.min(16) // max 16 bits per block for direct mode
}

/// Extract a value from the packed long array.
///
/// `bits` = bits per entry (1–64)
/// `index` = which entry to extract
///
/// Values are packed MSB-first within each long, across long boundaries.
fn extract_packed(data: &[i64], bits: u8, index: usize) -> u64 {
    if bits == 0 {
        return 0;
    }
    let bits = bits as u64;
    let bit_offset = index as u64 * bits;
    let long_idx = (bit_offset / 64) as usize;
    let shift = bit_offset % 64;

    let low = data[long_idx] as u64;
    let value = low >> shift;

    if shift + bits > 64 {
        // Spans two longs
        let high = *data.get(long_idx + 1).unwrap_or(&0) as u64;
        let mask = (1u64 << bits) - 1;
        (value | (high << (64 - shift))) & mask
    } else {
        value & ((1u64 << bits) - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_4bit() {
        // 4-bit entries: packed as [0x12, 0x34, 0x56, ...]
        // data[0] = 0x12345678_9ABCDEF0
        let data = vec![0x12345678_9ABCDEF0i64];
        assert_eq!(extract_packed(&data, 4, 0), 0x0);
        assert_eq!(extract_packed(&data, 4, 1), 0xF);
        assert_eq!(extract_packed(&data, 4, 2), 0xE);
        assert_eq!(extract_packed(&data, 4, 3), 0xD);
    }

    #[test]
    fn packed_across_boundary() {
        // 8-bit entries spanning across long boundary
        // data[0] = 0x...FF (bits 56-63 = 0xFF)
        // data[1] = 0xAB... (bits 0-7 = 0xAB)
        let data = vec![0xFF00_0000_0000_0000u64 as i64, 0xABu64 as i64];
        // Entry at index 7 (bits 56-63): should be 0xFF
        assert_eq!(extract_packed(&data, 8, 7), 0xFF);
        // Entry at index 8 (bits 64-71): spans data[1] bits 0-7 = 0xAB
        assert_eq!(extract_packed(&data, 8, 8), 0xAB);
    }

    #[test]
    fn packed_zero_bits() {
        assert_eq!(extract_packed(&[], 0, 42), 0);
    }
}
