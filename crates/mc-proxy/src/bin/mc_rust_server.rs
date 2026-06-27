//! mc-rust-server — Minecraft Java Edition 26.2 server in pure Rust.
//!
//! Usage:
//!   mc-rust-server [port=20067]

use anyhow::Result;
use mc_proxy::server::RustServer;

#[tokio::main]
async fn main() -> Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20067);

    RustServer::new(port).run().await
}
