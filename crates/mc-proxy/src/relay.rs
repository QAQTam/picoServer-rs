//! Bidirectional relay — pure TCP forwarding, no interception.
//! Config phase testing moved to standalone mc-config binary.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;

use bytes::Buf;
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;

use mc_core::VarInt;

use crate::config_handler;

static PLAY_PKT_N: AtomicU32 = AtomicU32::new(0);
static PLAY_C2S_N: AtomicU32 = AtomicU32::new(0);

const ST_HANDSHAKE: u8 = 0;
const ST_LOGIN: u8 = 1;

pub async fn bidirectional(
    id: u64,
    client: TcpStream,
    backend: TcpStream,
) -> anyhow::Result<(u64, u64)> {
    let (cr, cw) = tokio::io::split(client);
    let (br, bw) = tokio::io::split(backend);

    let state = Arc::new(AtomicU8::new(ST_HANDSHAKE));
    let compressed = Arc::new(AtomicBool::new(false));

    let c2s = tokio::spawn(relay_c2s(id, cr, bw, state.clone(), compressed.clone()));
    let s2c = tokio::spawn(relay_s2c(id, br, cw, state.clone(), compressed.clone()));

    let c2s = c2s.await??;
    let s2c = s2c.await??;
    Ok((c2s, s2c))
}

async fn relay_c2s(
    id: u64,
    mut src: ReadHalf<TcpStream>,
    mut dst: WriteHalf<TcpStream>,
    state: Arc<AtomicU8>,
    compressed: Arc<AtomicBool>,
) -> anyhow::Result<u64> {
    let mut buf = vec![0u8; 65536];
    let mut total: u64 = 0;
    let mut frame_buf = BytesMut::with_capacity(65536);

    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 { break; }
        total += n as u64;
        frame_buf.extend_from_slice(&buf[..n]);

        let comp = compressed.load(Ordering::Relaxed);
        while let Some((pid, pkt_data)) = config_handler::try_read_frame_compressed(&mut frame_buf, comp) {
            let current = state.load(Ordering::Relaxed);
            if current == ST_HANDSHAKE && pid == 0x00 {
                if let Ok(next) = parse_handshake_next(&pkt_data) {
                    if next == 2 {
                        state.store(ST_LOGIN, Ordering::Relaxed);
                        println!("[proxy c2s] state → Login");
                    }
                }
            }
            // Save C→S packets for protocol analysis
            let n = PLAY_C2S_N.fetch_add(1, Ordering::Relaxed);
            let path = format!("c2s_pkt_{n}_{id}_id0x{pid:02x}.bin");
            let _ = std::fs::write(&path, &pkt_data[..]);
            let hex: String = pkt_data.iter().take(40).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            println!("[proxy c2s] id=0x{pid:02x} body={}B: {hex}", pkt_data.len());
        }
        dst.write_all(&buf[..n]).await?;
    }
    Ok(total)
}

async fn relay_s2c(
    id: u64,
    mut src: ReadHalf<TcpStream>,
    mut dst: WriteHalf<TcpStream>,
    _state: Arc<AtomicU8>,
    compressed: Arc<AtomicBool>,
) -> anyhow::Result<u64> {
    let mut buf = vec![0u8; 65536];
    let mut total: u64 = 0;
    let mut frame_buf = BytesMut::with_capacity(65536);

    loop {
        let n = src.read(&mut buf).await?;
        if n == 0 { break; }
        total += n as u64;
        frame_buf.extend_from_slice(&buf[..n]);

        let comp = compressed.load(Ordering::Relaxed);
        while let Some((pid, pkt_data)) = config_handler::try_read_frame_compressed(&mut frame_buf, comp) {
            // Note: do NOT auto-detect compression here — 0x03 in Config is FinishConfiguration, not Set Compression.
            if pid != -1 {
                let hex: String = pkt_data.iter().take(60).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
                println!("[proxy s2c] id=0x{pid:02x} body={}B: {hex}", pkt_data.len());
                if pid == 0x0d && pkt_data.len() > 30000 {
                    let path = format!("update_tags_{id}.bin");
                    let _ = std::fs::write(&path, &pkt_data[..]);
                    println!("[proxy] saved UpdateTags to {path}");
                }
                // Save ALL S→C packets for protocol analysis (no limit)
                let n = PLAY_PKT_N.fetch_add(1, Ordering::Relaxed);
                let path = format!("play_pkt_{n}_{id}_id0x{pid:02x}.bin");
                let _ = std::fs::write(&path, &pkt_data[..]);
                if n == 0 {
                    println!("[proxy] saving all s2c packets to play_pkt_*_{id}_*.bin ...");
                }
            }
        }
        dst.write_all(&buf[..n]).await?;
    }
    Ok(total)
}

fn parse_handshake_next(data: &[u8]) -> anyhow::Result<i32> {
    let mut cursor = bytes::Bytes::copy_from_slice(data);
    let _pv = VarInt::read(&mut cursor)?;
    let addr_len = VarInt::read(&mut cursor)?.0 as usize;
    cursor.advance(addr_len);
    cursor.advance(2);
    Ok(VarInt::read(&mut cursor)?.0)
}


