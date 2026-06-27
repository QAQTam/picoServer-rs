//! Region file (.mca) reader.
//!
//! Each region file stores 32×32 chunks (1024 total). The 8 KiB header
//! contains location entries (4 bytes each) and timestamps (4 bytes each).
//!
//! ## Layout
//!
//! ```text
//! Header:  [location × 1024] [timestamp × 1024]  = 8 KiB
//! Chunks:  [length:u32][compression:u8][payload:NBT] ...
//! ```
//!
//! Each chunk is aligned to 4 KiB sectors.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use flate2::read::ZlibDecoder;
use thiserror::Error;

use crate::nbt::NbtValue;

/// Error during region file parsing.
#[derive(Debug, Error)]
pub enum RegionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("NBT error: {0}")]
    Nbt(#[from] crate::nbt::NbtError),
    #[error("invalid region header: file too small ({0} bytes)")]
    HeaderTooSmall(u64),
    #[error("chunk at ({x}, {z}) not present in region")]
    ChunkNotPresent { x: i32, z: i32 },
    #[error("unsupported compression type: {0}")]
    UnsupportedCompression(u8),
    #[error("chunk length mismatch: declared {declared}, actual {actual}")]
    LengthMismatch { declared: u32, actual: u32 },
}

/// A loaded chunk from a region file.
#[derive(Debug)]
pub struct RegionChunk {
    /// Absolute chunk X coordinate.
    pub chunk_x: i32,
    /// Absolute chunk Z coordinate.
    pub chunk_z: i32,
    /// The NBT root compound of the chunk.
    pub data: NbtValue,
    /// Last modification timestamp (Unix epoch).
    pub timestamp: u32,
}

/// Reader for a single `.mca` region file.
pub struct RegionFile {
    file: std::fs::File,
    /// Region base X (in chunks, so multiply by 32 for actual chunk coords).
    pub region_x: i32,
    /// Region base Z.
    pub region_z: i32,
    /// Raw location entries (1024 × u32 big-endian).
    locations: Vec<u32>,
    /// Raw timestamps (1024 × u32 big-endian).
    timestamps: Vec<u32>,
}

impl RegionFile {
    pub fn open(path: &Path) -> Result<Self, RegionError> {
        let filename = path.file_stem().unwrap().to_str().unwrap_or("");
        // Filename format: "r.X.Z.mca" where X, Z are region coordinates
        let parts: Vec<&str> = filename.split('.').collect();
        let (region_x, region_z) = if parts.len() >= 3 && parts[0] == "r" {
            (parts[1].parse::<i32>().unwrap_or(0), parts[2].parse::<i32>().unwrap_or(0))
        } else {
            (0, 0)
        };

        let mut file = std::fs::File::open(path)?;
        let file_len = file.metadata()?.len();
        if file_len < 8192 {
            return Err(RegionError::HeaderTooSmall(file_len));
        }

        // Read header
        let mut header = [0u8; 8192];
        file.read_exact(&mut header)?;

        let mut locations = Vec::with_capacity(1024);
        for i in 0..1024 {
            let off = i * 4;
            let val = u32::from_be_bytes([header[off], header[off + 1], header[off + 2], header[off + 3]]);
            locations.push(val);
        }

        let mut timestamps = Vec::with_capacity(1024);
        for i in 0..1024 {
            let off = 4096 + i * 4;
            let val = u32::from_be_bytes([header[off], header[off + 1], header[off + 2], header[off + 3]]);
            timestamps.push(val);
        }

        Ok(Self { file, region_x, region_z, locations, timestamps })
    }

    /// Check if a chunk exists in this region.
    pub fn has_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        let (lx, lz) = self.local_index(chunk_x, chunk_z);
        self.locations[lx + lz * 32] != 0
    }

    /// Load a chunk by absolute chunk coordinates.
    pub fn load_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> Result<RegionChunk, RegionError> {
        let (lx, lz) = self.local_index(chunk_x, chunk_z);
        let idx = lx + lz * 32;
        let loc = self.locations[idx];
        if loc == 0 {
            return Err(RegionError::ChunkNotPresent { x: chunk_x, z: chunk_z });
        }

        let offset = ((loc >> 8) as u64) * 4096;
        let _size = (loc & 0xFF) as u64 * 4096;

        self.file.seek(SeekFrom::Start(offset))?;
        let mut len_buf = [0u8; 4];
        self.file.read_exact(&mut len_buf)?;
        let declared_len = u32::from_be_bytes(len_buf);

        let mut comp_buf = [0u8; 1];
        self.file.read_exact(&mut comp_buf)?;
        let compression = comp_buf[0];

        let data_len = declared_len as usize;
        let mut raw = vec![0u8; data_len];
        self.file.read_exact(&mut raw)?;

        let decompressed = match compression {
            1 => {
                // GZip
                let mut decoder = flate2::read::GzDecoder::new(&raw[..]);
                let mut buf = Vec::new();
                decoder.read_to_end(&mut buf)?;
                buf
            }
            2 => {
                // Zlib
                let mut decoder = ZlibDecoder::new(&raw[..]);
                let mut buf = Vec::new();
                decoder.read_to_end(&mut buf)?;
                buf
            }
            3 => {
                // Uncompressed
                raw
            }
            other => return Err(RegionError::UnsupportedCompression(other)),
        };

        let (data, _) = NbtValue::read_compound(&decompressed)?;
        let timestamp = self.timestamps[idx];

        Ok(RegionChunk { chunk_x, chunk_z, data, timestamp })
    }

    /// Iterate over all chunk coordinates present in this region.
    pub fn chunk_coords(&self) -> Vec<(i32, i32)> {
        let mut coords = Vec::new();
        for lz in 0..32i32 {
            for lx in 0..32i32 {
                let idx = (lx + lz * 32) as usize;
                if self.locations[idx] != 0 {
                    let cx = self.region_x * 32 + lx;
                    let cz = self.region_z * 32 + lz;
                    coords.push((cx, cz));
                }
            }
        }
        coords
    }

    fn local_index(&self, chunk_x: i32, chunk_z: i32) -> (usize, usize) {
        let lx = (chunk_x - self.region_x * 32) as usize;
        let lz = (chunk_z - self.region_z * 32) as usize;
        (lx, lz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_filename_parsing() {
        // Test with a real file path
        let path = Path::new("C:/nonexistent/r.0.0.mca");
        // Can't actually read, but we can verify the coordinate parsing logic
        let filename = path.file_stem().unwrap().to_str().unwrap();
        let parts: Vec<&str> = filename.split('.').collect();
        assert_eq!(parts[0], "r");
        assert_eq!(parts[1], "0");
        assert_eq!(parts[2], "0");
    }
}
