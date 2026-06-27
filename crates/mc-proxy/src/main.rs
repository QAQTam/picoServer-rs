//! mc-proxy: TCP transparent proxy for Minecraft.
//!
//! Listens on `listen_addr` (default 0.0.0.0:20065), forwards every incoming
//! TCP connection to `backend_addr` (default 127.0.0.1:25565 — the Java server).
//!
//! Each forwarded connection is logged with per-direction byte counts and
//! optionally with per-packet summaries using mc-proto frame decoding.
//!
//! Usage:
//!   mc-proxy [listen_port=20065] [backend_port=25565]

use std::net::SocketAddr;

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};

use mc_proxy::relay;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,relay=debug")
        .init();

    let args: Vec<String> = std::env::args().collect();
    let listen_port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20065);
    let backend_port: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(25565);

    let listen_addr: SocketAddr = ([0, 0, 0, 0], listen_port).into();
    let backend_addr: SocketAddr = ([127, 0, 0, 1], backend_port).into();

    let listener = TcpListener::bind(listen_addr).await?;
    tracing::info!(
        "mc-proxy listening on {listen_addr} → backend {backend_addr} (protocol 776 / MC 26.2)"
    );

    let mut conn_id: u64 = 0;
    loop {
        let (client, peer) = listener.accept().await?;
        conn_id += 1;
        let id = conn_id;
        let backend = backend_addr;

        tokio::spawn(async move {
            tracing::info!(conn = id, %peer, "new connection");
            match handle(id, client, backend).await {
                Ok((c2s, s2c)) => {
                    tracing::info!(conn = id, c2s, s2c, "connection closed");
                }
                Err(e) => {
                    tracing::error!(conn = id, %e, "connection error");
                }
            }
        });
    }
}

async fn handle(
    id: u64,
    client: TcpStream,
    backend_addr: SocketAddr,
) -> Result<(u64, u64)> {
    let backend = TcpStream::connect(backend_addr).await?;
    tracing::debug!(conn = id, "connected to backend");

    let (c2s, s2c) = relay::bidirectional(id, client, backend).await?;
    Ok((c2s, s2c))
}
