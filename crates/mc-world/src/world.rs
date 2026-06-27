//! World container — lazy region loading, cross-chunk queries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::chunk::Chunk;
use crate::region::RegionFile;

/// A loaded Minecraft world, providing cross-chunk block queries.
pub struct World {
    /// Path to the world directory.
    path: PathBuf,
    /// Cached region files: key = (region_x, region_z).
    regions: HashMap<(i32, i32), RegionFile>,
}

impl World {
    /// Open a world directory.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
            regions: HashMap::new(),
        })
    }

    /// Get a chunk by absolute chunk coordinates.
    /// Loads the region file on demand and caches it.
    pub fn get_chunk(&mut self, chunk_x: i32, chunk_z: i32) -> Option<Chunk> {
        let rx = chunk_x >> 5;
        let rz = chunk_z >> 5;

        if !self.regions.contains_key(&(rx, rz)) {
            let path = self.region_path(rx, rz);
            if let Ok(rf) = RegionFile::open(&path) {
                self.regions.insert((rx, rz), rf);
            } else {
                return None;
            }
        }

        let region = self.regions.get_mut(&(rx, rz))?;
        match region.load_chunk(chunk_x, chunk_z) {
            Ok(rc) => Chunk::from_nbt(&rc.data),
            Err(_) => None,
        }
    }

    /// Get the block name at absolute world coordinates.
    /// Returns None if the chunk isn't loaded or the block is air.
    pub fn get_block_name(&mut self, world_x: i32, world_y: i32, world_z: i32) -> Option<String> {
        let cx = world_x >> 4;
        let cz = world_z >> 4;
        let chunk = self.get_chunk(cx, cz)?;
        let name = chunk.get_block(world_x, world_y, world_z);
        if name == "minecraft:air" || name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    }

    /// Get the top Y at given X,Z using heightmaps.
    pub fn get_top_y(&mut self, world_x: i32, world_z: i32) -> Option<i32> {
        let cx = world_x >> 4;
        let cz = world_z >> 4;
        let lx = (world_x & 15) as u8;
        let lz = (world_z & 15) as u8;
        self.get_chunk(cx, cz)?.get_top_y(lx, lz)
    }

    fn region_path(&self, rx: i32, rz: i32) -> PathBuf {
        self.path
            .join("dimensions/minecraft/overworld/region")
            .join(format!("r.{rx}.{rz}.mca"))
    }
}
