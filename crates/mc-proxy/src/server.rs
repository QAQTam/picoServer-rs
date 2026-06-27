//! Minecraft Rust Server — core connection handler.
//!
//! Handles the full client lifecycle:
//!   Handshake → Login → Config → Play (with chunk loading + per-tick loop).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::Result;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::select;
use uuid::Uuid;

use mc_core::{VarInt, VarLong};

/// Track mined block positions so changes survive reconnect.
static BLOCK_CHANGES: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

use crate::chunk::{LevelChunk, get_drop_item_id};
use crate::config_handler::{self, PKT_SB_FINISH_CONFIGURATION};
use crate::login_packet::LoginPacket;
use crate::packet_ids::*;
use crate::server_state::ServerState;

/// Read Minecraft string from Bytes buffer.
fn read_string(buf: &mut Bytes) -> Result<String> {
    let len = VarInt::read(buf)?.0 as usize;
    if buf.remaining() < len {
        anyhow::bail!("string too short");
    }
    let bytes = buf.split_to(len);
    Ok(String::from_utf8(bytes.to_vec())?)
}

/// Write Minecraft string to BytesMut.
fn write_string(buf: &mut BytesMut, s: &str) {
    let b = s.as_bytes();
    VarInt(b.len() as i32).write(buf);
    buf.extend_from_slice(b);
}

/// Build an AddEntity (0x01) packet body for MC 26.2 (protocol 776).
///
/// Format:
///   VarInt entity_id
///   UUID (16 bytes)
///   VarInt entity_type
///   f64 x, y, z
///   u8 pitch, yaw, head_yaw (angle = byte * 360 / 256)
///   VarInt data (entity-specific; 0 = no extra data)
///   bool hasVelocity (1 byte)
///   optional short velX, velY, velZ (if hasVelocity)
#[allow(dead_code)]
fn write_add_entity(
    entity_id: i32,
    uuid: u128,
    entity_type: i32,
    x: f64, y: f64, z: f64,
    pitch: u8, yaw: u8, head_yaw: u8,
    data: i32,
    has_velocity: bool,
) -> BytesMut {
    let mut buf = BytesMut::new();
    VarInt(entity_id).write(&mut buf);
    buf.put_u128(uuid);
    VarInt(entity_type).write(&mut buf);
    buf.put_f64(x);
    buf.put_f64(y);
    buf.put_f64(z);
    buf.put_u8(pitch);
    buf.put_u8(yaw);
    buf.put_u8(head_yaw);
    VarInt(data).write(&mut buf);
    buf.put_u8(if has_velocity { 1 } else { 0 });
    if has_velocity {
        buf.put_i16(0); buf.put_i16(0); buf.put_i16(0);
    }
    buf
}

async fn send_chunks(
    socket: &mut TcpStream,
    state: &mut ServerState,
    chunks: &[(i32, i32)],
) -> Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }
    let framed = config_handler::frame_packet(CB_CHUNK_BATCH_START, &[], false);
    socket.write_all(&framed).await?;
    for &(cx, cz) in chunks {
        let body = LevelChunk::empty(cx, cz).to_bytes();
        let framed = config_handler::frame_packet(CB_LEVEL_CHUNK_WITH_LIGHT, &body, false);
        socket.write_all(&framed).await?;
    }
    let mut bf = BytesMut::new();
    VarInt(chunks.len() as i32).write(&mut bf);
    let framed = config_handler::frame_packet(CB_CHUNK_BATCH_FINISHED, &bf, false);
    socket.write_all(&framed).await?;
    state.mark_chunks_loaded(chunks);
    let (pcx, pcz) = state.player_chunk();
    println!("[play] → sent {} chunks around ({},{})", chunks.len(), pcx, pcz);
    Ok(())
}

/// The main Minecraft server instance.
pub struct RustServer {
    pub port: u16,
}

impl RustServer {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub async fn run(self) -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter("info")
            .init();

        // Print block palette from superflat template for reference
        let palette = crate::chunk::parse_template_block_palette();
        if palette.is_empty() {
            println!("[world] template parse failed, using fallback block→item map");
        } else {
            println!("[world] superflat block palette:");
            let mut ys: Vec<_> = palette.keys().copied().collect();
            ys.sort();
            for y in ys {
                println!("[world]   y={}: block_state_ids={:?}", y, &palette[&y]);
            }
        }

        let bind: SocketAddr = ([0, 0, 0, 0], self.port).into();
        let listener = TcpListener::bind(bind).await?;
        println!("mc-rust-server listening on 0.0.0.0:{}", self.port);

        loop {
            let (mut socket, peer) = listener.accept().await?;
            println!("[server] connection from {peer}");
            tokio::spawn(async move {
                if let Err(e) = handle_connection(&mut socket).await {
                    eprintln!("[server] {peer} error: {e}");
                }
            });
        }
    }
}

async fn handle_connection(socket: &mut TcpStream) -> Result<()> {
    let mut buf = BytesMut::with_capacity(4096);
    let mut phase = "handshake";
    let mut player_uuid: u128 = 0;
    let compression = false;

    loop {
        let n = socket.read_buf(&mut buf).await?;
        if n == 0 { break; }
        while let Some((frame_id, frame_data)) = config_handler::try_read_frame_compressed(&mut buf, compression) {
            match phase {
                "handshake" => {
                    if frame_id != 0x00 {
                        anyhow::bail!("expected Handshake, got 0x{frame_id:02x}");
                    }
                    let mut data = frame_data.clone();
                    let proto = VarInt::read(&mut data)?.0;
                    let addr = read_string(&mut data)?;
                    let port = data.get_u16();
                    let next = VarInt::read(&mut data)?.0;
                    println!("[server] handshake: proto={proto}, {addr}:{port}, next={next}");
                    if next == 2 {
                        phase = "login";
                    } else {
                        anyhow::bail!("expected next_state=2 (Login), got {next}");
                    }
                }
                "login" => {
                    if frame_id == 0x00 {
                        let mut data = frame_data.clone();
                        let name = read_string(&mut data)?;
                        let uuid = Uuid::from_u128(data.get_u128());
                        player_uuid = uuid.as_u128();
                        println!("[server] login: {name} ({uuid})");

                        let mut body = BytesMut::new();
                        body.put_u128(uuid.as_u128());
                        write_string(&mut body, &name);
                        VarInt(0i32).write(&mut body);
                        body.put_u128(Uuid::new_v4().as_u128());
                        let framed = config_handler::frame_packet(0x02, &body, compression);
                        socket.write_all(&framed).await?;

                        let mut bb = BytesMut::new();
                        write_string(&mut bb, "minecraft:brand");
                        write_string(&mut bb, "vanilla");
                        let framed = config_handler::frame_packet(0x01, &bb, compression);
                        socket.write_all(&framed).await?;
                    } else if frame_id == 0x03 {
                        phase = "config";
                        let regs = config_handler::build_registry_packets();
                        let n = regs.len();
                        println!("[server] sending {n} registry packets");
                        for (i, (pid, body)) in regs.iter().enumerate() {
                            let framed = config_handler::frame_packet(*pid, body, compression);
                            if i == 0 || i >= n.saturating_sub(3) {
                                let hex: String = framed.iter().take(32).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
                                println!("[server]   pkt[{i}] id=0x{pid:02x} ({})B: {hex}...", framed.len());
                            }
                            socket.write_all(&framed).await?;
                        }
                        println!("[server] → all registry packets sent");
                    }
                }
                "config" => {
                    if frame_id == PKT_SB_FINISH_CONFIGURATION {
                        println!("[server] ✓ Config phase complete → PLAY");
                        run_play_phase(socket, player_uuid).await?;
                        return Ok(());
                    } else if frame_id == 0x00 {
                        println!("[server] ← client packet id=0x{frame_id:02x} ({}B body)", frame_data.len());
                    } else {
                        println!("[server] ← client packet id=0x{frame_id:02x} ({}B body)", frame_data.len());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

async fn run_play_phase(socket: &mut TcpStream, player_uuid: u128) -> Result<()> {
    let mut state = ServerState::new(player_uuid);

    // Load captured packet templates for the initial burst (optional)
    let proxy_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut pkt_map: std::collections::BTreeMap<u32, (i32, Vec<u8>)> = std::collections::BTreeMap::new();
    if let Ok(dir) = std::fs::read_dir(proxy_dir) {
        for entry in dir.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("play_pkt_") && name.ends_with(".bin") {
                let rest = &name[9..];
                if let Some(underscore_pos) = rest.find('_') {
                    let n_str = &rest[..underscore_pos];
                    if let Ok(n) = n_str.parse::<u32>() {
                        if n >= 35 {
                            if let Some(id_pos) = rest.find("id0x") {
                                let id_rest = &rest[id_pos+4..];
                                if let Some(dot_pos) = id_rest.find('.') {
                                    if let Ok(pid) = i32::from_str_radix(&id_rest[..dot_pos], 16) {
                                        if let Ok(body) = std::fs::read(entry.path()) {
                                            pkt_map.insert(n, (pid, body));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut play_packets: Vec<(i32, Vec<u8>)> = pkt_map.into_values().take(213).collect();
    println!("[play] loaded {} Play packets from proxy capture", play_packets.len());

    // If no captured packets exist, build a minimal initial burst dynamically
    if play_packets.is_empty() {
        println!("[play] no templates found, using dynamic initial burst");
        // Send LoginPacket first
        let login = LoginPacket {
            entity_id: 1352, hardcore: false,
            dimensions: vec!["minecraft:overworld".to_string(), "minecraft:the_end".to_string(), "minecraft:the_nether".to_string()],
            max_players: 20, view_distance: 10, sim_distance: 10,
            reduced_debug_info: false, enable_respawn_screen: true, do_limited_crafting: false,
            dimension_type: String::new(), dimension_name: "minecraft:overworld".to_string(),
            seed: 9152277222534345964i64, game_type: 0, previous_game_type: 127,
            is_debug: false, is_flat: false, death_location: None,
            portal_cooldown: 63, sea_level: 0, envelope_follows: false,
        };
        let framed = config_handler::frame_packet(CB_LOGIN, &login.to_bytes(), false);
        socket.write_all(&framed).await?;

        // Critical initial burst packets — send them in order
        for &pid in &[CB_CHANGE_DIFFICULTY, CB_SET_HELD_SLOT, CB_GAME_EVENT,
            CB_PLAYER_POSITION, CB_SET_TIME, CB_TICKING_STATE, CB_TICKING_STEP,
            CB_PLAYER_ABILITIES, CB_RECIPE_BOOK_SETTINGS, CB_SERVER_DATA,
            CB_PLAYER_INFO_UPDATE, CB_INITIALIZE_BORDER, CB_SET_DEFAULT_SPAWN_POSITION,
            CB_SET_EXPERIENCE, CB_SET_HEALTH]
        {
            send_initial_packet(socket, &mut state, pid, None).await?;
        }

        // Send initial chunks
        let new_chunks = state.visible_chunks();
        send_chunks(&mut *socket, &mut state, &new_chunks).await?;
        let mut cc = BytesMut::new();
        VarInt(0i32).write(&mut cc); VarInt(0i32).write(&mut cc);
        let framed = config_handler::frame_packet(CB_SET_CHUNK_CACHE_CENTER, &cc, false);
        socket.write_all(&framed).await?;

        // Enter play loop
        return finish_play_start(socket, state).await;
    }

    // Replace LoginPacket with dynamic version
    if play_packets[0].0 == CB_LOGIN {
        let login = LoginPacket {
            entity_id: 1352, hardcore: false,
            dimensions: vec![
                "minecraft:overworld".to_string(),
                "minecraft:the_end".to_string(),
                "minecraft:the_nether".to_string(),
            ],
            max_players: 20, view_distance: 10, sim_distance: 10,
            reduced_debug_info: false, enable_respawn_screen: true, do_limited_crafting: false,
            dimension_type: String::new(), dimension_name: "minecraft:overworld".to_string(),
            seed: 9152277222534345964i64, game_type: 0, previous_game_type: 127,
            is_debug: false, is_flat: false, death_location: None,
            portal_cooldown: 63, sea_level: 0, envelope_follows: false,
        };
        play_packets[0] = (0x31, login.to_bytes().to_vec());
    }

    // Send initial burst with dynamic substitutions
    for (i, (pid, body)) in play_packets.iter().enumerate() {
        send_initial_packet(socket, &mut state, *pid, Some(body)).await?;
        if i < 5 || body.len() > 100 {
            println!("[play] → pkt[{}] id=0x{pid:02x} ({}B)", i, body.len());
        }
    }

    // Initial chunk load + block replay + play loop
    finish_play_start(socket, state).await
}

/// Send a single packet during the initial burst.
/// If `template_body` is Some, it's used as fallback for non-dynamic packets.
async fn send_initial_packet(socket: &mut TcpStream, state: &mut ServerState, pid: i32, template_body: Option<&[u8]>) -> Result<()> {
    match pid {
        pid if pid == CB_LOGIN => {} // already handled
        CB_CHANGE_DIFFICULTY => {
            let mut dd = BytesMut::new(); dd.put_u8(1); dd.put_u8(0);
            let framed = config_handler::frame_packet(CB_CHANGE_DIFFICULTY, &dd, false);
            socket.write_all(&framed).await?;
        }
        CB_SET_HELD_SLOT => {
            let mut hs = BytesMut::new(); VarInt(0i32).write(&mut hs);
            let framed = config_handler::frame_packet(CB_SET_HELD_SLOT, &hs, false);
            socket.write_all(&framed).await?;
        }
        CB_GAME_EVENT => {
            let mut ge = BytesMut::new(); ge.put_u8(13); ge.put_f32(0.0);
            let framed = config_handler::frame_packet(CB_GAME_EVENT, &ge, false);
            socket.write_all(&framed).await?;
        }
        CB_PLAYER_POSITION => {
            let tid = state.next_teleport_id();
            let mut pp = BytesMut::new();
            VarInt(tid).write(&mut pp);
            pp.put_f64(state.player.x); pp.put_f64(state.player.y); pp.put_f64(state.player.z);
            pp.put_f64(0.0); pp.put_f64(0.0); pp.put_f64(0.0);
            pp.put_f32(state.player.yaw); pp.put_f32(state.player.pitch);
            pp.put_i32(0);
            let framed = config_handler::frame_packet(CB_PLAYER_POSITION, &pp, false);
            socket.write_all(&framed).await?;
        }
        CB_SET_TIME => {
            let mut tb = BytesMut::new(); tb.put_i64(0i64); tb.put_u8(0x00);
            let framed = config_handler::frame_packet(CB_SET_TIME, &tb, false);
            socket.write_all(&framed).await?;
        }
        CB_TICKING_STATE => {
            let mut ts = BytesMut::new(); ts.put_f32(20.0); ts.put_u8(0);
            let framed = config_handler::frame_packet(CB_TICKING_STATE, &ts, false);
            socket.write_all(&framed).await?;
        }
        CB_TICKING_STEP => {
            let mut ts = BytesMut::new(); VarInt(0i32).write(&mut ts);
            let framed = config_handler::frame_packet(CB_TICKING_STEP, &ts, false);
            socket.write_all(&framed).await?;
        }
        CB_PLAYER_ABILITIES => {
            let mut pa = BytesMut::new(); pa.put_u8(4); pa.put_f32(0.05); pa.put_f32(0.1);
            let framed = config_handler::frame_packet(CB_PLAYER_ABILITIES, &pa, false);
            socket.write_all(&framed).await?;
        }
        CB_RECIPE_BOOK_SETTINGS => {
            let framed = config_handler::frame_packet(CB_RECIPE_BOOK_SETTINGS, &[0,0,0,0,0,0,0,0], false);
            socket.write_all(&framed).await?;
        }
        CB_SERVER_DATA => {
            let text = b"RustMC Server";
            let mut sd = BytesMut::new(); sd.put_u8(8); sd.put_u8(0);
            VarInt(text.len() as i32).write(&mut sd); sd.extend_from_slice(text); sd.put_u8(0);
            let framed = config_handler::frame_packet(CB_SERVER_DATA, &sd, false);
            socket.write_all(&framed).await?;
        }
        CB_PLAYER_INFO_UPDATE => {
            let framed = config_handler::frame_packet(CB_PLAYER_INFO_UPDATE, &[0, 0], false);
            socket.write_all(&framed).await?;
        }
        CB_COMMANDS | CB_UPDATE_RECIPES | CB_RECIPE_BOOK_ADD
            | CB_ENTITY_EVENT | CB_ADD_ENTITY | CB_SET_ENTITY_DATA
            | CB_UPDATE_ATTRIBUTES | CB_UPDATE_ADVANCEMENTS
            | CB_CONTAINER_SET_CONTENT | CB_SET_EQUIPMENT => {
            // Skip — not critical or handled elsewhere
        }
        CB_CHUNK_BATCH_START | CB_SET_CHUNK_CACHE_CENTER | CB_LEVEL_CHUNK_WITH_LIGHT => {
            // Skip — send_chunks handles these
        }
        CB_INITIALIZE_BORDER => {
            let mut b = BytesMut::new(); b.put_f64(0.0); b.put_f64(0.0);
            b.put_f64(60000000.0); b.put_f64(60000000.0); VarLong(0i64).write(&mut b);
            VarInt(29999984i32).write(&mut b); VarInt(5i32).write(&mut b); VarInt(300i32).write(&mut b);
            let framed = config_handler::frame_packet(CB_INITIALIZE_BORDER, &b, false);
            socket.write_all(&framed).await?;
        }
        CB_SET_DEFAULT_SPAWN_POSITION => {
            let mut b = BytesMut::new();
            write_string(&mut b, "minecraft:overworld");
            let bx: i64 = 0; let by: i64 = -60; let bz: i64 = 0;
            let pos: i64 = ((bx & 0x3FFFFFF) << 38) | ((bz & 0x3FFFFFF) << 12) | (by & 0xFFF);
            b.put_i64(pos); b.put_f32(0.0); b.extend_from_slice(&[0u8; 4]);
            let framed = config_handler::frame_packet(CB_SET_DEFAULT_SPAWN_POSITION, &b, false);
            socket.write_all(&framed).await?;
        }
        CB_SET_EXPERIENCE => {
            let mut b = BytesMut::new(); b.put_f32(0.0); VarInt(0i32).write(&mut b); VarInt(0i32).write(&mut b);
            let framed = config_handler::frame_packet(CB_SET_EXPERIENCE, &b, false);
            socket.write_all(&framed).await?;
        }
        CB_SET_HEALTH => {
            let mut b = BytesMut::new(); b.put_f32(20.0); VarInt(20i32).write(&mut b); b.put_f32(2.0);
            let framed = config_handler::frame_packet(CB_SET_HEALTH, &b, false);
            socket.write_all(&framed).await?;
        }
        CB_BUNDLE => {
            let framed = config_handler::frame_packet(CB_BUNDLE, &[], false);
            socket.write_all(&framed).await?;
        }
        _ => {
            // Fallback: send template if available, skip if not
            if let Some(body) = template_body {
                let framed = config_handler::frame_packet(pid, body, false);
                socket.write_all(&framed).await?;
            }
        }
    }
    Ok(())
}

/// Finish play startup: chunks + block replay + enter main loop.
async fn finish_play_start(socket: &mut TcpStream, mut state: ServerState) -> Result<()> {
    // Initial chunk load
    let initial_chunks = state.visible_chunks();
    send_chunks(socket, &mut state, &initial_chunks).await?;
    {
        let (cx, cz) = state.last_chunk;
        let mut cc = BytesMut::new();
        VarInt(cx).write(&mut cc); VarInt(cz).write(&mut cc);
        let framed = config_handler::frame_packet(CB_SET_CHUNK_CACHE_CENTER, &cc, false);
        socket.write_all(&framed).await?;
    }

    // Replay all previously mined blocks
    {
        let positions: Vec<i64> = {
            BLOCK_CHANGES.lock().unwrap().iter().copied().collect()
        };
        let count = positions.len();
        for pos in positions {
            let mut bu = BytesMut::new();
            bu.put_i64(pos);
            VarInt(0i32).write(&mut bu);
            let framed = config_handler::frame_packet(CB_BLOCK_UPDATE, &bu, false);
            socket.write_all(&framed).await?;
        }
        if count > 0 {
            println!("[play] → replayed {} block changes", count);
        }
    }

    // Play loop: read client packets + per-tick timer
    let mut play_buf = BytesMut::with_capacity(65536);
    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));

    loop {
        select! {
            result = socket.read_buf(&mut play_buf) => {
                let n = result?;
                if n == 0 { break; }
                while let Some((pid, mut body)) =
                    config_handler::try_read_frame_compressed(&mut play_buf, false)
                {
                    handle_sb_packet(pid, &mut body, &mut state);
                    // Send pending block updates
                    while let Some((pos, block)) = state.pending_block_updates.pop() {
                        let mut bu = BytesMut::new();
                        bu.put_i64(pos);
                        VarInt(block).write(&mut bu);
                        let framed = config_handler::frame_packet(CB_BLOCK_UPDATE, &bu, false);
                        socket.write_all(&framed).await?;
                    }
                    // Send pending entity spawns (in FIFO order: AddEntity first, then SetEntityData)
                    for (pid, body) in state.pending_spawns.drain(..) {
                        let framed = config_handler::frame_packet(pid, &body, false);
                        socket.write_all(&framed).await?;
                    }
                }
                if state.chunk_changed() {
                    let new_chunks = state.new_visible_chunks();
                    if !new_chunks.is_empty() {
                        send_chunks(&mut *socket, &mut state, &new_chunks).await?;
                        let (cx, cz) = state.last_chunk;
                        let mut cc = BytesMut::new();
                        VarInt(cx).write(&mut cc); VarInt(cz).write(&mut cc);
                        let framed = config_handler::frame_packet(CB_SET_CHUNK_CACHE_CENTER, &cc, false);
                        socket.write_all(&framed).await?;
                    }
                }
            }
            _ = tick_interval.tick() => {
                state.advance_tick();
                send_per_tick(socket, &mut state).await?;
            }
        }
    }
    println!("[play] client disconnected after {}s ({} ticks)", state.uptime_secs(), state.tick_count);
    Ok(())
}

fn handle_sb_packet(pid: i32, body: &mut Bytes, state: &mut ServerState) {
    match pid {
        SB_CLIENT_TICK_END => { state.advance_tick(); }
        SB_ACCEPT_TELEPORTATION | SB_CHUNK_BATCH_RECEIVED | SB_MOVE_PLAYER_STATUS_ONLY => {}
        SB_MOVE_PLAYER_POS => {
            if body.len() >= 25 {
                state.player.x = body.get_f64();
                state.player.y = body.get_f64();
                state.player.z = body.get_f64();
                let _ = body.get_u8();
            }
        }
        SB_MOVE_PLAYER_POS_ROT => {
            if body.len() >= 33 {
                state.player.x = body.get_f64();
                state.player.y = body.get_f64();
                state.player.z = body.get_f64();
                state.player.yaw = body.get_f32();
                state.player.pitch = body.get_f32();
                let _ = body.get_u8();
            }
        }
        SB_MOVE_PLAYER_ROT => {
            if body.len() >= 9 {
                state.player.yaw = body.get_f32();
                state.player.pitch = body.get_f32();
                let _ = body.get_u8();
            }
        }
        SB_PLAYER_LOADED => { println!("[play] ← PlayerLoaded"); }
        SB_SWING => {
            let hand = VarInt::read(body).unwrap_or(VarInt(0)).0;
            println!("[play] ← Swing hand={}", hand);
        }
        SB_CLIENT_COMMAND => {
            let a = VarInt::read(body).unwrap_or(VarInt(0)).0;
            let name = match a { 0 => "RESPAWN", 1 => "STATS", _ => "?" };
            println!("[play] ← ClientCommand {name}");
        }
        SB_PLAYER_ACTION => {
            let a = VarInt::read(body).unwrap_or(VarInt(0)).0;
            let name = match a { 0=>"START_DIG", 1=>"CANCEL_DIG", 2=>"FINISH_DIG", 3=>"DROP_ALL", 4=>"DROP_ITEM", 5=>"RELEASE_USE", 6=>"SWAP_HANDS", _=>"?" };
            let pos = body.get_i64(); let face = body.get_i8();
            let _seq = VarInt::read(body).unwrap_or(VarInt(0)).0;
            let x = (pos >> 38) as i32;
            let y = (pos << 52 >> 52) as i32;
            let z = ((pos >> 12) & 0x3FFFFFF) as i32;
            if a == 2 {
                state.pending_block_updates.push((pos, 0));
                BLOCK_CHANGES.lock().unwrap().insert(pos);
                // Spawn item entity at broken block
                let eid = state.next_entity_id;
                state.next_entity_id += 1;
                let ex = x as f64 + 0.5;
                let ey = y as f64 + 0.5;
                let ez = z as f64 + 0.5;
                let uuid = Uuid::new_v4().as_u128();
                let mut add_body = BytesMut::new();
                VarInt(eid).write(&mut add_body);
                add_body.put_u128(uuid);
                VarInt(71i32).write(&mut add_body); // entity_type = minecraft:item
                add_body.put_f64(ex); add_body.put_f64(ey); add_body.put_f64(ez);
                add_body.put_u8(0); add_body.put_u8(0); add_body.put_u8(0);
                VarInt(0i32).write(&mut add_body); // data = 0
                add_body.put_u8(0); // hasVelocity = false
                state.pending_spawns.push((CB_ADD_ENTITY, add_body));
                let item_id = get_drop_item_id(y);
                // SetEntityData: ItemStack = item at broken block
                let mut meta = BytesMut::new();
                VarInt(eid).write(&mut meta);
                meta.put_u8(8);              // index
                VarInt(7i32).write(&mut meta);  // type = item_stack
                VarInt(1i32).write(&mut meta);  // count = 1
                VarInt(item_id).write(&mut meta); // item_id
                VarInt(0i32).write(&mut meta);  // add_count = 0 (no components to add)
                VarInt(0i32).write(&mut meta);  // remove_count = 0 (no components to remove)
                meta.put_u8(0xff);       // end of metadata
                state.pending_spawns.push((CB_SET_ENTITY_DATA, meta));
                // Track item entity for pickup detection
                state.item_entities.push(crate::server_state::ItemEntity {
                    entity_id: eid,
                    item_id,
                    count: 1,
                    x: ex,
                    y: ey,
                    z: ez,
                });
            }
            println!("[play] ← PlayerAction {name} ({x},{y},{z}) face={face}");
        }
        SB_USE_ITEM_ON => {
            let hand = VarInt::read(body).unwrap_or(VarInt(0)).0;
            let pos = body.get_i64();
            let face = VarInt::read(body).unwrap_or(VarInt(0)).0;
            let _cx = body.get_f32(); let _cy = body.get_f32(); let _cz = body.get_f32();
            let inside = body.get_u8() != 0;
            let _seq = VarInt::read(body).unwrap_or(VarInt(0)).0;
            let x = (pos >> 38) as i32;
            let y = (pos << 52 >> 52) as i32;
            let z = ((pos >> 12) & 0x3FFFFFF) as i32;
            // Decrement held item when placing a block
            let slot = state.held_slot as usize;
            if let Some(stack) = &mut state.inventory[slot] {
                stack.count -= 1;
                if stack.count <= 0 {
                    state.inventory[slot] = None;
                }
            }
            println!("[play] ← UseItemOn hand={hand} ({x},{y},{z}) face={face} inside={inside}");
        }
        SB_SET_CARRIED_ITEM => {
            let slot = VarInt::read(body).unwrap_or(VarInt(0)).0 as u16;
            state.held_slot = slot;
        }
        SB_KEEP_ALIVE => {
            let id = VarInt::read(body).unwrap_or(VarInt(0)).0;
            if id % 20 == 0 { println!("[play] ← KeepAlive id={id}"); }
        }
        SB_CHAT_COMMAND => {
            if let Ok(msg) = read_string(body) {
                println!("[play] ← ChatCommand: {msg}");
            }
        }
        _ => {}
    }
}

async fn send_per_tick(socket: &mut TcpStream, state: &mut ServerState) -> Result<()> {
    // KeepAlive (0x2c): i64
    let mut kb = BytesMut::new();
    kb.put_i64(state.tick_count as i64);
    let framed = config_handler::frame_packet(CB_KEEP_ALIVE, &kb, false);
    socket.write_all(&framed).await?;

    // SetTime (0x71): i64 + 0x00
    let mut tb = BytesMut::new();
    tb.put_i64(state.tick_count as i64);
    tb.put_u8(0x00);
    let framed = config_handler::frame_packet(CB_SET_TIME, &tb, false);
    socket.write_all(&framed).await?;

    // TickingState (0x7f): f32(20.0) + bool(false)
    let mut ts = BytesMut::new();
    ts.put_f32(20.0); ts.put_u8(0);
    let framed = config_handler::frame_packet(CB_TICKING_STATE, &ts, false);
    socket.write_all(&framed).await?;

    // TickingStep (0x80): VarInt(0)
    let mut tstep = BytesMut::new();
    VarInt(0i32).write(&mut tstep);
    let framed = config_handler::frame_packet(CB_TICKING_STEP, &tstep, false);
    socket.write_all(&framed).await?;

    if state.tick_count % 200 == 0 {
        println!("[play] alive: {}s, {} ticks, pos=({:.1}, {:.1}, {:.1})",
            state.uptime_secs(), state.tick_count,
            state.player.x, state.player.y, state.player.z);
    }

    // Item pickup detection — check every tick (50ms)
    if !state.item_entities.is_empty() {
        let px = state.player.x;
        let py = state.player.y;
        let pz = state.player.z;
        let pickup_range = 4.0_f64;

        let mut picked_up = Vec::new();
        for (i, entity) in state.item_entities.iter().enumerate() {
            let dx = entity.x - px;
            let dy = entity.y - py;
            let dz = entity.z - pz;
            let dist_sq = dx * dx + dy * dy + dz * dz;
            if dist_sq < pickup_range * pickup_range {
                picked_up.push(i);
            }
        }

        if !picked_up.is_empty() {
            println!("[play] pickup: {} items within range", picked_up.len());
        }

        // Process pickups in reverse order to remove correctly
        for &idx in picked_up.iter().rev() {
            let entity = &state.item_entities[idx];
            let eid = entity.entity_id;
            let item_id = entity.item_id;
            let count = entity.count;

            // Add to player inventory
            let mut remaining = count;
            for slot in 0..36 {  // hotbar (0-8) + main (9-35)
                if remaining <= 0 { break; }
                if let Some(stack) = &mut state.inventory[slot] {
                    if stack.item_id == item_id && stack.count < 64 {
                        let add = (64 - stack.count).min(remaining);
                        stack.count += add;
                        remaining -= add;
                    }
                } else {
                    let add = remaining.min(64);
                    state.inventory[slot] = Some(crate::server_state::ItemStack {
                        item_id, count: add,
                    });
                    remaining -= add;
                }
            }

            if remaining == count {
                continue; // no space — skip
            }

            // Send Take Item Entity (0x7C)
            let mut take = BytesMut::new();
            VarInt(eid).write(&mut take);
            VarInt(state.player.entity_id).write(&mut take);
            VarInt(count - remaining).write(&mut take);
            let framed = config_handler::frame_packet(CB_TAKE_ITEM_ENTITY, &take, false);
            if let Err(e) = socket.write_all(&framed).await {
                println!("[play] pickup send error: {e}");
            }

            println!("[play] picked up item {} (item_id={})", eid, item_id);
        }

        // Send SetContainerSlot for changed slots
        // Player inventory window (windowId=0) slot mapping:
        //   0=craft_out, 1-4=craft_grid, 5-8=armor, 9-35=main, 36-44=hotbar, 45=offhand
        // Our internal storage: 0-8=hotbar, 9-35=main
        // Map internal slot -> window slot: hotbar(0-8)->36-44, main(9-35)->9-35
        if !picked_up.is_empty() {
            // Collect changed slots and send updates
            let mut slots: Vec<u16> = Vec::new();
            // Find non-empty inventory slots to sync
            for internal_slot in 0..36 {
                if state.inventory[internal_slot].is_some() {
                    let window_slot = if internal_slot < 9 {
                        internal_slot as u16 + 36 // hotbar: 0-8 -> 36-44
                    } else {
                        internal_slot as u16      // main: 9-35 -> 9-35
                    };
                    slots.push(window_slot);
                }
            }
            if !slots.is_empty() {
                state.container_state_id += 1;
                for &window_slot in &slots {
                    let mut slot_data = BytesMut::new();
                    slot_data.put_u8(0); // windowId = 0 (player inventory)
                    VarInt(state.container_state_id).write(&mut slot_data);
                    slot_data.put_i16(window_slot as i16);
                    // Map back to internal slot
                    let internal_slot = if window_slot >= 36 {
                        (window_slot - 36) as usize
                    } else {
                        window_slot as usize
                    };
                    if let Some(stack) = &state.inventory[internal_slot] {
                        VarInt(stack.count).write(&mut slot_data);
                        VarInt(stack.item_id).write(&mut slot_data);
                        VarInt(0i32).write(&mut slot_data); // add_count
                        VarInt(0i32).write(&mut slot_data); // remove_count
                    } else {
                        VarInt(0i32).write(&mut slot_data); // empty
                    }
                    let framed = config_handler::frame_packet(CB_CONTAINER_SET_SLOT, &slot_data, false);
                    let _ = socket.write_all(&framed).await;
                }
            }
        }

        // Remove picked up entities (reverse order)
        for &idx in picked_up.iter().rev() {
            state.item_entities.remove(idx);
        }
    }
    Ok(())
}
