//! Server List Ping responder — standalone binary.
//!
//! Listens on 0.0.0.0:20066, responds to Minecraft Server List Ping
//! (Handshake → Status → Ping/Pong).
//! If a client tries to log in, sends a friendly disconnect.
//!
//! Usage:
//!   cargo run --bin mc-ping [port=20066]

use std::net::SocketAddr;

use anyhow::Result;
use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use mc_proto::handshake::Handshake;
use mc_proto::login::{LoginDisconnect, LoginStart};
use mc_proto::packet::{self, Packet};
use mc_proto::state::State;
use mc_proto::status::{PingRequest, PongResponse, ServerStatus, StatusRequest, StatusResponse};

#[tokio::main]
async fn main() -> Result<()> {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20066);

    let bind: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = TcpListener::bind(bind).await?;
    println!("RustMC Ping listening on 0.0.0.0:{port} (protocol 776 / MC 26.2)");

    loop {
        let (mut socket, peer) = listener.accept().await?;
        println!("[ping] connection from {peer}");

        tokio::spawn(async move {
            if let Err(e) = handle(&mut socket).await {
                eprintln!("[ping] {peer} error: {e}");
            }
        });
    }
}

async fn handle(socket: &mut tokio::net::TcpStream) -> Result<()> {
    let mut buf = BytesMut::with_capacity(4096);
    let mut state = State::Handshake;

    loop {
        let n = socket.read_buf(&mut buf).await?;
        if n == 0 {
            break; // EOF
        }

        while let Some(frame) = packet::try_read_frame(&mut buf)? {
            match state {
                State::Handshake => {
                    if frame.id != Handshake::ID {
                        anyhow::bail!("expected Handshake (0x00), got 0x{:02x}", frame.id);
                    }
                    let mut data = frame.data.clone();
                    let hs = Handshake::read(&mut data)?;
                    println!(
                        "[ping] handshake: proto={}, addr={}:{}, next={:?}",
                        hs.protocol_version, hs.server_address, hs.server_port, hs.next_state
                    );
                    state = hs.next_state;
                }
                State::Status => match frame.id {
                    StatusRequest::ID => {
                        let _req = StatusRequest::read(&mut frame.data.clone())?;
                        let status = ServerStatus::new(0, 20, "§aRustMC §f26.2 §8| §7Powered by Rust");
                        let resp = StatusResponse { json: status.to_json() };
                        let mut body = BytesMut::new();
                        resp.write(&mut body);
                        let framed = packet::encode_packet(StatusResponse::ID, &body);
                        socket.write_all(&framed).await?;
                        println!("[ping] → StatusResponse");
                    }
                    PingRequest::ID => {
                        let mut data = frame.data.clone();
                        let ping = PingRequest::read(&mut data)?;
                        let pong = PongResponse { payload: ping.payload };
                        let mut body = BytesMut::new();
                        pong.write(&mut body);
                        let framed = packet::encode_packet(PongResponse::ID, &body);
                        socket.write_all(&framed).await?;
                        println!("[ping] → Pong({})", ping.payload);
                        break;
                    }
                    other => {
                        anyhow::bail!("unexpected packet 0x{other:02x} in Status state");
                    }
                },
                State::Login => {
                    // Client is trying to join — read Login Start, then kick
                    if frame.id == LoginStart::ID {
                        let mut data = frame.data.clone();
                        let ls = LoginStart::read(&mut data)?;
                        println!("[ping] login start from '{}' (uuid={})", ls.name, ls.uuid);
                        let disconnect = LoginDisconnect {
                            reason: r#"{"text":"§cThis is a ping server. Join the real server on port 25565!","color":"red"}"#.into(),
                        };
                        let mut body = BytesMut::new();
                        disconnect.write(&mut body);
                        let framed = packet::encode_packet(LoginDisconnect::ID, &body);
                        socket.write_all(&framed).await?;
                        println!("[ping] → LoginDisconnect (redirect to 25565)");
                        break;
                    } else {
                        anyhow::bail!("unexpected packet 0x{:02x} in Login state", frame.id);
                    }
                }
                _ => {
                    anyhow::bail!("unexpected state {state:?} for ping");
                }
            }
        }
    }

    Ok(())
}
