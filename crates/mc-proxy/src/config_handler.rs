//! Configuration phase handler — sends registry data from Rust.

use bytes::Buf;
use bytes::BytesMut;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use mc_core::VarInt;
use std::io::Read;
use std::io::Write;

use crate::registry_data;

/// 26.2 Configuration phase packet IDs (verified from real server proxy capture).
pub const PKT_FINISH_CONFIGURATION: i32 = 0x03;
pub const PKT_REGISTRY_DATA: i32 = 0x07;
pub const PKT_UPDATE_ENABLED_FEATURES: i32 = 0x0c;
pub const PKT_UPDATE_TAGS: i32 = 0x0d;
pub const PKT_SELECT_KNOWN_PACKS: i32 = 0x0e;
pub const PKT_SB_FINISH_CONFIGURATION: i32 = 0x03; // C→S

/// Build all Registry Data packets for the Configuration phase.
/// Order must match real Minecraft 26.2 server: Features → KnownPacks → Registries → Tags → Finish
pub fn build_registry_packets() -> Vec<(i32, BytesMut)> {
    let mut packets = Vec::new();

    // Update Enabled Features (0x0c) — "minecraft:vanilla"
    let mut fb = BytesMut::new();
    VarInt(1i32).write(&mut fb);
    write_string(&mut fb, "minecraft:vanilla");
    packets.push((PKT_UPDATE_ENABLED_FEATURES, fb));

    // Select Known Packs (0x0e) — "minecraft:core@26.2"
    let mut pb = BytesMut::new();
    VarInt(1i32).write(&mut pb);
    write_string(&mut pb, "minecraft");
    write_string(&mut pb, "core");
    write_string(&mut pb, "26.2");
    packets.push((PKT_SELECT_KNOWN_PACKS, pb));

    // All registry data packets (0x07)
    for (key, entries) in registry_data::registry_entries() {
        let mut body = BytesMut::new();
        write_string(&mut body, key);
        VarInt(entries.len() as i32).write(&mut body);
        for entry_id in entries {
            write_string(&mut body, entry_id);
            VarInt(0i32).write(&mut body);
        }
        packets.push((PKT_REGISTRY_DATA, body));
    }

    // Update Tags (0x0d) — embedded from real server capture
    let mut tb = BytesMut::new();
    tb.extend_from_slice(include_bytes!("update_tags.bin"));
    packets.push((PKT_UPDATE_TAGS, tb));

    // Finish Configuration (0x03) — empty packet, signals end of config phase
    packets.push((PKT_FINISH_CONFIGURATION, BytesMut::new()));

    packets
}

fn write_string(buf: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    VarInt(bytes.len() as i32).write(buf);
    buf.extend_from_slice(bytes);
}

/// Encode a complete frame.
pub fn frame_packet(packet_id: i32, body: &[u8], compression: bool) -> BytesMut {
    let id_varint = VarInt(packet_id);
    let id_len = id_varint.encoded_len();

    if compression {
        let total = id_len + body.len();
        if total >= 256 {
            let mut uncompressed = BytesMut::with_capacity(total);
            id_varint.write(&mut uncompressed);
            uncompressed.extend_from_slice(body);

            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&uncompressed).expect("zlib write");
            let compressed = encoder.finish().expect("zlib finish");

            let data_len_varint = VarInt(total as i32);
            let outer_len = data_len_varint.encoded_len() + compressed.len();
            let mut buf =
                BytesMut::with_capacity(VarInt(outer_len as i32).encoded_len() + outer_len);
            VarInt(outer_len as i32).write(&mut buf);
            data_len_varint.write(&mut buf);
            buf.extend_from_slice(&compressed);
            buf
        } else {
            let data_len_varint = VarInt(0i32);
            let outer_len = data_len_varint.encoded_len() + id_len + body.len();
            let mut buf =
                BytesMut::with_capacity(VarInt(outer_len as i32).encoded_len() + outer_len);
            VarInt(outer_len as i32).write(&mut buf);
            data_len_varint.write(&mut buf);
            id_varint.write(&mut buf);
            buf.extend_from_slice(body);
            buf
        }
    } else {
        let outer_len = id_len + body.len();
        let mut buf = BytesMut::with_capacity(VarInt(outer_len as i32).encoded_len() + outer_len);
        VarInt(outer_len as i32).write(&mut buf);
        id_varint.write(&mut buf);
        buf.extend_from_slice(body);
        buf
    }
}

/// Read a complete frame from buffer, with optional compression support.
/// Returns (packet_id, body_bytes) or None if not enough data.
pub fn try_read_frame_compressed(buf: &mut bytes::BytesMut, compressed: bool) -> Option<(i32, bytes::Bytes)> {
    if buf.is_empty() {
        return None;
    }
    let peek = buf.clone().freeze();
    let (length, len_bytes) = VarInt::from_bytes(&peek).ok()?;
    let length = length.0 as usize;
    if peek.len() < len_bytes + length {
        return None;
    }
    buf.advance(len_bytes);
    let raw_body = buf.split_to(length).freeze();

    if compressed {
        let (data_len, dl_bytes) = VarInt::from_bytes(&raw_body).ok()?;
        let dl = data_len.0 as usize;
        let inner = &raw_body[dl_bytes..];
        if dl == 0 {
            let mut body = bytes::Bytes::copy_from_slice(inner);
            let (pid_varint, _) = VarInt::from_bytes(&body).ok()?;
            let pid = pid_varint.0;
            body.advance(pid_varint.encoded_len());
            Some((pid, body))
        } else {
            // decompress
            let mut decoder = ZlibDecoder::new(inner);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed).ok()?;
            let mut body = bytes::Bytes::from(decompressed);
            let (pid_varint, _) = VarInt::from_bytes(&body).ok()?;
            let pid = pid_varint.0;
            body.advance(pid_varint.encoded_len());
            Some((pid, body))
        }
    } else {
        let mut body = raw_body.clone();
        let (pid_varint, _) = VarInt::from_bytes(&body).ok()?;
        let pid = pid_varint.0;
        body.advance(pid_varint.encoded_len());
        Some((pid, body))
    }
}
