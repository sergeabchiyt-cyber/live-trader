//! RFC 6455 WebSocket framing (server side) — handshake accept-key, frame
//! encode (unmasked server->client), frame decode (masked client->server,
//! ping/pong/close handling). Parity with the Python stdlib implementation,
//! plus a server-initiated 30s ping keepalive (fixes idle-proxy timeouts).

use std::io::{Read, Write};

pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Sec-WebSocket-Accept = base64(SHA1(key + GUID))
pub fn accept_key(key: &str) -> String {
    use ring::digest::{digest, SHA1_FOR_LEGACY_USE_ONLY};
    let combined = format!("{}{}", key, WS_GUID);
    let d = digest(&SHA1_FOR_LEGACY_USE_ONLY, combined.as_bytes());
    b64_encode(d.as_ref())
}

fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Encode a server->client frame (FIN=1, no mask).
pub fn encode_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let n = payload.len();
    let mut out = Vec::with_capacity(n + 10);
    out.push(0x80 | opcode);
    if n < 126 {
        out.push(n as u8);
    } else if n < 65536 {
        out.push(126);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(n as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

pub enum ClientFrame {
    #[allow(dead_code)] // client->server text is ignored by protocol
    Text(String),
    Ping(Vec<u8>),
    Pong,
    Close(Vec<u8>),
}

/// Read one client frame (blocking). Ok(None) = EOF / clean end.
pub fn read_frame<R: Read>(r: &mut R) -> std::io::Result<Option<ClientFrame>> {
    let mut hdr = [0u8; 2];
    match r.read_exact(&mut hdr) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let opcode = hdr[0] & 0x0F;
    let masked = hdr[1] & 0x80 != 0;
    let mut len = (hdr[1] & 0x7F) as u64;
    if len == 126 {
        let mut b = [0u8; 2];
        r.read_exact(&mut b)?;
        len = u16::from_be_bytes(b) as u64;
    } else if len == 127 {
        let mut b = [0u8; 8];
        r.read_exact(&mut b)?;
        len = u64::from_be_bytes(b);
    }
    if len > 1_048_576 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        r.read_exact(&mut payload)?;
    }
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some(match opcode {
        0x8 => ClientFrame::Close(payload),
        0x9 => ClientFrame::Ping(payload),
        0xA => ClientFrame::Pong,
        _ => ClientFrame::Text(String::from_utf8_lossy(&payload).into_owned()),
    }))
}

pub fn write_frame<W: Write>(w: &mut W, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
    w.write_all(&encode_frame(opcode, payload))?;
    w.flush()
}
