//! World inspection tool.
//!
//! Reads a Minecraft 26.x world and prints summary information.
//!
//! Usage:
//!   mc-world-inspect <world_path> [region_x] [region_z]

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use mc_world::region::RegionFile;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let world_path = args.get(1).expect("Usage: mc-world-inspect <world_path> [region_x] [region_z]");

    let world = Path::new(world_path);
    let overworld = world.join("dimensions/minecraft/overworld/region");

    if !overworld.exists() {
        anyhow::bail!("Region directory not found: {}", overworld.display());
    }

    println!("World: {world_path}");
    println!("Region dir: {}", overworld.display());

    // Read level.dat (GZip-compressed)
    let level_dat_path = world.join("level.dat");
    if level_dat_path.exists() {
        let raw = std::fs::read(&level_dat_path)?;
        let mut decoder = flate2::read::GzDecoder::new(&raw[..]);
        let mut data = Vec::new();
        decoder.read_to_end(&mut data)?;
        match mc_world::nbt::NbtValue::read_compound(&data) {
            Ok((root, _)) => {
                let data_entry = root.get_compound("Data");
                if let Some(data) = data_entry {
                    if let Some(name) = data.iter().find(|(k, _)| k == "LevelName")
                        .and_then(|(_, v)| match v { mc_world::nbt::NbtValue::String(s) => Some(s.as_str()), _ => None })
                    {
                        println!("Level name: {name}");
                    }
                    if let Some(ver) = data.iter().find(|(k, _)| k == "DataVersion")
                        .and_then(|(_, v)| match v { mc_world::nbt::NbtValue::Int(i) => Some(*i), _ => None })
                    {
                        println!("Data version: {ver}");
                    }
                }
            }
            Err(e) => eprintln!("Warning: could not parse level.dat: {e}"),
        }
    }

    // If specific region requested
    if let (Some(rx_str), Some(rz_str)) = (args.get(2), args.get(3)) {
        let rx: i32 = rx_str.parse()?;
        let rz: i32 = rz_str.parse()?;
        inspect_region(&overworld, rx, rz)?;
    } else {
        // List all regions
        list_regions(&overworld)?;
    }

    Ok(())
}

fn list_regions(dir: &Path) -> Result<()> {
    let mut regions: Vec<(i32, i32, u64)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().unwrap_or("");
        if name.ends_with(".mca") {
            let parts: Vec<&str> = name.split('.').collect();
            if parts.len() >= 3 && parts[0] == "r" {
                let rx = parts[1].parse::<i32>().unwrap_or(0);
                let rz = parts[2].parse::<i32>().unwrap_or(0);
                let size = entry.metadata()?.len();
                regions.push((rx, rz, size));
            }
        }
    }
    regions.sort_by_key(|(rx, rz, _)| (*rx, *rz));

    println!("\n{} region files:", regions.len());
    let total_size: u64 = regions.iter().map(|(_, _, s)| s).sum();
    println!("Total size: {} MB", total_size / 1024 / 1024);

    for (rx, rz, size) in &regions {
        let chunks = count_chunks_in_region(dir, *rx, *rz)?;
        println!("  r.{rx}.{rz}.mca  {size:>8} bytes  {chunks} chunks");
        if regions.len() > 50 {
            println!("  ... ({} more regions)", regions.len() - 51);
            break;
        }
    }

    Ok(())
}

fn count_chunks_in_region(dir: &Path, rx: i32, rz: i32) -> Result<usize> {
    let path = dir.join(format!("r.{rx}.{rz}.mca"));
    let mut region = RegionFile::open(&path)?;
    Ok(region.chunk_coords().len())
}

fn inspect_region(dir: &Path, rx: i32, rz: i32) -> Result<()> {
    let path = dir.join(format!("r.{rx}.{rz}.mca"));
    println!("\nInspecting: {}", path.display());

    let mut region = RegionFile::open(&path)?;
    let coords = region.chunk_coords();
    println!("Chunks present: {}", coords.len());

    for (cx, cz) in coords.iter().take(5) {
        match region.load_chunk(*cx, *cz) {
            Ok(chunk) => {
                let parsed = mc_world::chunk::Chunk::from_nbt(&chunk.data);
                match parsed {
                    Some(c) => {
                        println!(
                            "  Chunk ({}, {})  status={}  sections={}  block_entities={}  min_y={}",
                            c.x, c.z, c.status, c.sections.len(), c.block_entities.len(), c.min_y
                        );
                        // Show heightmap summary
                        for (name, data) in &c.heightmaps {
                            println!("    Heightmap {name}: {} longs", data.len());
                            // Show a few surface heights
                            if name == "MOTION_BLOCKING" || name == "WORLD_SURFACE" {
                                for lx in (0..16).step_by(4) {
                                    let y = c.get_top_y(lx, 0).unwrap_or(0);
                                    print!(" {y}");
                                }
                                println!();
                            }
                        }
                        for sec in &c.sections {
                            let non_air = sec.palette.iter().filter(|b| b.name != "minecraft:air").count();
                            println!(
                                "    Section Y={}  palette={} blocks ({} non-air)  bpb={}  biomes={}",
                                sec.y, sec.palette.len(), non_air, sec.bits_per_block, sec.biome_palette.len()
                            );
                        }
                        for be in &c.block_entities {
                            println!("    BlockEntity: {} at ({}, {}, {})", be.id, be.x, be.y, be.z);
                        }
                        // Demo: query a few blocks using get_block
                        let x = c.x * 16;
                        let z = c.z * 16;
                        for &(lx, ly, lz) in &[(0, -60, 0), (0, -52, 0), (0, 64, 0), (8, -56, 8)] {
                            let name = c.get_block(x + lx, ly, z + lz);
                            if name != "minecraft:air" {
                                println!("    Block at (~{}, {}, ~{}): {name}", x + lx, ly, z + lz);
                            }
                        }
                        // Count total non-air blocks via iterator
                        let count = c.iter_blocks().count();
                        println!("    Total non-air blocks: {count}");
                    }
                    None => println!("  Chunk ({}, {}) — failed to parse", cx, cz),
                }
            }
            Err(e) => println!("  Chunk ({}, {}) — error: {e}", cx, cz),
        }
    }

    if coords.len() > 5 {
        println!("  ... ({} more chunks)", coords.len() - 5);
    }

    Ok(())
}
