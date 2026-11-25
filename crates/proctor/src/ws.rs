//! Zero-dependency RFC 6455 WebSocket server protocol implementation.
//!
//! Provides the HTTP Upgrade handshake (`Sec-WebSocket-Accept` using an
//! embedded RFC 3174 SHA-1 and RFC 4648 Base64 encoder), server-to-client
//! frame writers, client-to-server masked frame reading, and the binary
//! media packet wire format used by the proctor dashboard.

use std::io::{self, Read, Write};
use std::net::TcpStream;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const OPCODE_CONTINUATION: u8 = 0x0;
const OPCODE_TEXT: u8 = 0x1;
const OPCODE_BINARY: u8 = 0x2;
const OPCODE_CLOSE: u8 = 0x8;
const OPCODE_PING: u8 = 0x9;
const OPCODE_PONG: u8 = 0xA;

const FIN_BIT: u8 = 0x80;
const MASK_BIT: u8 = 0x80;

/// Binary media frame kind identifier for webcam JPEG frames.
pub const MEDIA_KIND_WEBCAM: u8 = 0;

/// Binary media frame kind identifier for screen-capture JPEG frames.
pub const MEDIA_KIND_SCREEN: u8 = 1;

/// Incoming WebSocket frame decoded from a client stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsIncoming {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Close,
}

/// Computes the RFC 6455 `Sec-WebSocket-Accept` header value from a client's
/// `Sec-WebSocket-Key`.
pub fn compute_accept_key(sec_websocket_key: &str) -> String {
    let mut input = String::with_capacity(sec_websocket_key.len() + WS_GUID.len());
    input.push_str(sec_websocket_key.trim());
    input.push_str(WS_GUID);
    let digest = sha1(input.as_bytes());
    base64_encode(&digest)
}

/// Writes the HTTP `101 Switching Protocols` WebSocket upgrade response.
pub fn send_handshake(stream: &mut TcpStream, sec_websocket_key: &str) -> io::Result<()> {
    let accept = compute_accept_key(sec_websocket_key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n"
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// Writes an unmasked server-to-client WebSocket text frame (`0x81`).
pub fn write_text_frame(stream: &mut TcpStream, text: &str) -> io::Result<()> {
    write_frame(stream, FIN_BIT | OPCODE_TEXT, text.as_bytes())
}

/// Writes an unmasked server-to-client WebSocket binary frame (`0x82`).
pub fn write_binary_frame(stream: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    write_frame(stream, FIN_BIT | OPCODE_BINARY, payload)
}

/// Writes an unmasked server-to-client WebSocket pong frame (`0x8A`).
///
/// Per RFC 6455 Section 5.5, control frame payloads are capped at 125 bytes.
pub fn write_pong_frame(stream: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    let capped = &payload[..payload.len().min(125)];
    write_frame(stream, FIN_BIT | OPCODE_PONG, capped)
}

/// Writes an unmasked server-to-client WebSocket close frame (`0x88`).
pub fn write_close_frame(stream: &mut TcpStream) -> io::Result<()> {
    write_frame(stream, FIN_BIT | OPCODE_CLOSE, &[])
}

/// Encodes a binary media packet with layout:
/// `[kind: u8][id_len: u8][student_id bytes (truncated to <= 255)][jpeg bytes]`.
pub fn encode_media_packet(kind: u8, student_id: &str, jpeg: &[u8]) -> Vec<u8> {
    let id_bytes = student_id.as_bytes();
    let mut id_len = id_bytes.len().min(255);
    while id_len > 0 && !student_id.is_char_boundary(id_len) {
        id_len -= 1;
    }

    let mut packet = Vec::with_capacity(2 + id_len + jpeg.len());
    packet.push(kind);
    packet.push(id_len as u8);
    packet.extend_from_slice(&id_bytes[..id_len]);
    packet.extend_from_slice(jpeg);
    packet
}

/// Reads and decodes a WebSocket frame from `stream`, unmasking client frames
/// and enforcing `max_payload`.
///
/// Unsolicited Pong frames are ignored and skipped. Fragmented text/binary
/// messages are reassembled while enforcing `max_payload` on total length.
pub fn read_frame(stream: &mut impl Read, max_payload: usize) -> io::Result<WsIncoming> {
    let mut frag_opcode: Option<u8> = None;
    let mut frag_buf: Vec<u8> = Vec::new();

    loop {
        let (fin, opcode, payload) = read_single_frame(stream, max_payload)?;

        match opcode {
            OPCODE_CLOSE => return Ok(WsIncoming::Close),
            OPCODE_PING => return Ok(WsIncoming::Ping(payload)),
            OPCODE_PONG => continue,
            OPCODE_TEXT | OPCODE_BINARY => {
                if frag_opcode.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "received new data frame while fragmented message was in progress",
                    ));
                }
                if fin {
                    return finish_data_frame(opcode, payload);
                }
                frag_opcode = Some(opcode);
                frag_buf = payload;
            }
            OPCODE_CONTINUATION => {
                let Some(initial_opcode) = frag_opcode else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "received continuation frame without initial fragment",
                    ));
                };
                if frag_buf.len().saturating_add(payload.len()) > max_payload {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "reassembled WebSocket payload exceeds max_payload ({max_payload})"
                        ),
                    ));
                }
                frag_buf.extend_from_slice(&payload);
                if fin {
                    let data = std::mem::take(&mut frag_buf);
                    return finish_data_frame(initial_opcode, data);
                }
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsupported WebSocket opcode: 0x{other:x}"),
                ));
            }
        }
    }
}

fn finish_data_frame(opcode: u8, payload: Vec<u8>) -> io::Result<WsIncoming> {
    match opcode {
        OPCODE_TEXT => String::from_utf8(payload)
            .map(WsIncoming::Text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        OPCODE_BINARY => Ok(WsIncoming::Binary(payload)),
        _ => unreachable!(),
    }
}

fn read_single_frame(stream: &mut impl Read, max_payload: usize) -> io::Result<(bool, u8, Vec<u8>)> {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header)?;

    let fin = (header[0] & FIN_BIT) != 0;
    let opcode = header[0] & 0x0F;
    let masked = (header[1] & MASK_BIT) != 0;
    let len_code = header[1] & 0x7F;

    let payload_len: usize = match len_code {
        0..=125 => len_code as usize,
        126 => {
            let mut ext = [0u8; 2];
            stream.read_exact(&mut ext)?;
            u16::from_be_bytes(ext) as usize
        }
        127 => {
            let mut ext = [0u8; 8];
            stream.read_exact(&mut ext)?;
            let raw = u64::from_be_bytes(ext);
            if raw > max_payload as u64 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("WebSocket payload length {raw} exceeds max_payload ({max_payload})"),
                ));
            }
            usize::try_from(raw).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "WebSocket payload length overflow")
            })?
        }
        _ => unreachable!(),
    };

    if payload_len > max_payload {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("WebSocket payload length {payload_len} exceeds max_payload ({max_payload})"),
        ));
    }

    let mask_key = if masked {
        let mut key = [0u8; 4];
        stream.read_exact(&mut key)?;
        Some(key)
    } else {
        None
    };

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream.read_exact(&mut payload)?;
    }

    if let Some(key) = mask_key {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[i & 3];
        }
    }

    Ok((fin, opcode, payload))
}

fn write_frame(stream: &mut impl Write, first_byte: u8, payload: &[u8]) -> io::Result<()> {
    let mut header = [0u8; 10];
    header[0] = first_byte;

    let header_len = if payload.len() < 126 {
        header[1] = payload.len() as u8;
        2
    } else if payload.len() <= u16::MAX as usize {
        header[1] = 126;
        header[2..4].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        4
    } else {
        header[1] = 127;
        header[2..10].copy_from_slice(&(payload.len() as u64).to_be_bytes());
        10
    };

    stream.write_all(&header[..header_len])?;
    if !payload.is_empty() {
        stream.write_all(payload)?;
    }
    stream.flush()
}

/// RFC 3174 SHA-1 digest (used solely for the RFC 6455 WebSocket handshake).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = Vec::with_capacity((data.len() + 9 + 63) & !63);
    msg.extend_from_slice(data);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 80];
    for chunk in msg.chunks_exact(64) {
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDCu32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

/// Standard RFC 4648 Base64 encoder with `=` padding.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::net::TcpListener;

    fn build_masked_frame(opcode: u8, mask: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.push(FIN_BIT | opcode);
        if payload.len() < 126 {
            frame.push(MASK_BIT | (payload.len() as u8));
        } else if payload.len() <= u16::MAX as usize {
            frame.push(MASK_BIT | 126);
            frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        } else {
            frame.push(MASK_BIT | 127);
            frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
        frame.extend_from_slice(&mask);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i & 3]);
        }
        frame
    }

    #[test]
    fn rfc6455_handshake_accept_key_vector() {
        // RFC 6455 Section 1.3 test vector
        assert_eq!(
            compute_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        // Leading/trailing HTTP header whitespace should be trimmed cleanly
        assert_eq!(
            compute_accept_key("  dGhlIHNhbXBsZSBub25jZQ== \r\n"),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn send_handshake_writes_101_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        let mut client = TcpStream::connect(addr).expect("connect");
        let (mut server, _) = listener.accept().expect("accept");

        send_handshake(&mut server, "dGhlIHNhbXBsZSBub25jZQ==").expect("send_handshake");
        drop(server);

        let mut received = String::new();
        client.read_to_string(&mut received).expect("read response");
        assert_eq!(
            received,
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
             \r\n"
        );
    }

    #[test]
    fn read_masked_text_ping_and_close_frames() {
        let mask = [0x37, 0xFA, 0x21, 0x3D];
        let mut wire = Vec::new();
        wire.extend_from_slice(&build_masked_frame(OPCODE_TEXT, mask, b"hello proctor"));
        wire.extend_from_slice(&build_masked_frame(OPCODE_PING, [1, 2, 3, 4], b"hb"));
        wire.extend_from_slice(&build_masked_frame(OPCODE_CLOSE, [9, 8, 7, 6], &[]));

        let mut cursor = Cursor::new(wire);
        assert_eq!(
            read_frame(&mut cursor, 1024).expect("text frame"),
            WsIncoming::Text("hello proctor".to_string())
        );
        assert_eq!(
            read_frame(&mut cursor, 1024).expect("ping frame"),
            WsIncoming::Ping(b"hb".to_vec())
        );
        assert_eq!(
            read_frame(&mut cursor, 1024).expect("close frame"),
            WsIncoming::Close
        );
    }

    #[test]
    fn read_frame_enforces_max_payload() {
        let frame = build_masked_frame(OPCODE_BINARY, [0xAA, 0xBB, 0xCC, 0xDD], &[42u8; 200]);
        let mut cursor = Cursor::new(frame);
        let err = read_frame(&mut cursor, 128).expect_err("should reject oversized frame");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn roundtrip_text_and_binary_frames_across_length_tiers_over_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local_addr");
        let mut client = TcpStream::connect(addr).expect("connect");
        let (mut server, _) = listener.accept().expect("accept");

        let small_text = "small payload";
        let medium_text = "m".repeat(300); // 126..=65535 (16-bit extended length)
        let large_binary: Vec<u8> = (0..70_000).map(|i| (i % 251) as u8).collect(); // >= 65536 (64-bit length)

        let writer = std::thread::spawn(move || {
            write_text_frame(&mut server, small_text).expect("write small text");
            write_text_frame(&mut server, &medium_text).expect("write medium text");
            write_binary_frame(&mut server, &large_binary).expect("write large binary");
            write_pong_frame(&mut server, b"pong-ok").expect("write pong");
            write_close_frame(&mut server).expect("write close");
        });

        assert_eq!(
            read_frame(&mut client, 100_000).expect("read small text"),
            WsIncoming::Text(small_text.to_string())
        );
        assert_eq!(
            read_frame(&mut client, 100_000).expect("read medium text"),
            WsIncoming::Text("m".repeat(300))
        );
        let expected_bin: Vec<u8> = (0..70_000).map(|i| (i % 251) as u8).collect();
        assert_eq!(
            read_frame(&mut client, 100_000).expect("read large binary"),
            WsIncoming::Binary(expected_bin)
        );
        // Pong frame is skipped automatically by read_frame, yielding the trailing Close frame.
        assert_eq!(
            read_frame(&mut client, 100_000).expect("read close after pong"),
            WsIncoming::Close
        );

        writer.join().expect("writer thread");
    }

    #[test]
    fn encode_media_packet_layout_and_truncation() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let pkt = encode_media_packet(MEDIA_KIND_WEBCAM, "stu-42", &jpeg);
        assert_eq!(pkt[0], MEDIA_KIND_WEBCAM);
        assert_eq!(pkt[1], 6);
        assert_eq!(&pkt[2..8], b"stu-42");
        assert_eq!(&pkt[8..], &jpeg);

        // Long student ID is truncated to 255 bytes.
        let long_id = "x".repeat(300);
        let pkt_long = encode_media_packet(MEDIA_KIND_SCREEN, &long_id, &jpeg);
        assert_eq!(pkt_long[0], MEDIA_KIND_SCREEN);
        assert_eq!(pkt_long[1], 255);
        assert_eq!(&pkt_long[2..257], "x".repeat(255).as_bytes());
        assert_eq!(&pkt_long[257..], &jpeg);
    }
}
