use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustbrowser_core::{
    HeartbeatResponse, ProctorCommand, RoomPolicyOverrides, StudentHeartbeat, StudentStatus,
};
use url::Url;

#[derive(Debug, Clone)]
pub struct TelemetryState {
    pub status: StudentStatus,
    pub active_host: String,
    pub violations: Vec<String>,
}

#[cfg(target_os = "macos")]
fn https_post(
    url_str: &str,
    content_type: &str,
    room_code: Option<&str>,
    join_token: Option<&str>,
    body: &[u8],
    timeout: Duration,
) -> Result<(i64, Vec<u8>), String> {
    use std::ffi::c_void;
    use std::sync::mpsc;

    use block2::RcBlock;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    let (tx, rx) = mpsc::channel::<Result<(i64, Vec<u8>), String>>();
    let tx_slot = Arc::new(Mutex::new(Some(tx)));

    let task: Retained<AnyObject> = autoreleasepool(|_| -> Result<Retained<AnyObject>, String> {
        let ns_url_str = NSString::from_str(url_str);
        // SAFETY: `NSURL` is a standard Foundation class; `URLWithString:` accepts `&NSString`.
        let ns_url: Option<Retained<AnyObject>> =
            unsafe { msg_send![class!(NSURL), URLWithString: &*ns_url_str] };
        let ns_url = ns_url.ok_or_else(|| format!("invalid HTTPS URL: {url_str}"))?;

        let timeout_secs: f64 = timeout.as_secs_f64().max(0.1);
        // cachePolicy = 1 (`NSURLRequestReloadIgnoringLocalCacheData`).
        // SAFETY: `NSMutableURLRequest` accepts valid `NSURL`, `NSUInteger`, and `NSTimeInterval`.
        let req: Retained<AnyObject> = unsafe {
            msg_send![
                class!(NSMutableURLRequest),
                requestWithURL: &*ns_url,
                cachePolicy: 1usize,
                timeoutInterval: timeout_secs
            ]
        };

        let method = NSString::from_str("POST");
        let ct_val = NSString::from_str(content_type);
        let ct_key = NSString::from_str("Content-Type");
        // SAFETY: `req` is a valid `NSMutableURLRequest` and all string args are valid `NSString`.
        unsafe {
            let (): () = msg_send![&*req, setHTTPMethod: &*method];
            let (): () = msg_send![&*req, setValue: &*ct_val, forHTTPHeaderField: &*ct_key];
        }

        if let Some(code) = room_code {
            let clean = code.replace(['\r', '\n'], "");
            let val = NSString::from_str(&clean);
            let key = NSString::from_str("X-Room-Code");
            // SAFETY: `req` is a valid `NSMutableURLRequest` and args are valid `NSString`.
            unsafe {
                let (): () = msg_send![&*req, setValue: &*val, forHTTPHeaderField: &*key];
            }
        }
        if let Some(token) = join_token {
            let clean = token.replace(['\r', '\n'], "");
            let val = NSString::from_str(&clean);
            let key = NSString::from_str("X-Room-Token");
            // SAFETY: `req` is a valid `NSMutableURLRequest` and args are valid `NSString`.
            unsafe {
                let (): () = msg_send![&*req, setValue: &*val, forHTTPHeaderField: &*key];
            }
        }

        if !body.is_empty() {
            // SAFETY: `dataWithBytes:length:` copies `body.len()` bytes from `body.as_ptr()`.
            let ns_data: Retained<AnyObject> = unsafe {
                msg_send![
                    class!(NSData),
                    dataWithBytes: body.as_ptr().cast::<c_void>(),
                    length: body.len()
                ]
            };
            // SAFETY: `req` is a valid `NSMutableURLRequest` and `ns_data` is a valid `NSData`.
            unsafe {
                let (): () = msg_send![&*req, setHTTPBody: &*ns_data];
            }
        }

        let tx_for_block = Arc::clone(&tx_slot);
        let block = RcBlock::new(
            move |data: *mut AnyObject, response: *mut AnyObject, error: *mut AnyObject| {
                let outcome = if !error.is_null() {
                    // SAFETY: `error` is a non-null `NSError*` provided by `NSURLSession`.
                    let desc: Option<Retained<NSString>> =
                        unsafe { msg_send![error, localizedDescription] };
                    Err(desc
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "NSURLSession request failed".to_string()))
                } else {
                    let status: i64 = if !response.is_null() {
                        // SAFETY: `response` is a non-null `NSURLResponse*`.
                        let is_http: bool = unsafe {
                            msg_send![response, isKindOfClass: class!(NSHTTPURLResponse)]
                        };
                        if is_http {
                            // SAFETY: Verified `NSHTTPURLResponse` responds to `statusCode`.
                            let code: isize = unsafe { msg_send![response, statusCode] };
                            code as i64
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let resp_bytes = if !data.is_null() {
                        // SAFETY: `data` is a non-null `NSData*`.
                        let len: usize = unsafe { msg_send![data, length] };
                        if len > 0 {
                            let ptr: *const u8 = unsafe { msg_send![data, bytes] };
                            if !ptr.is_null() {
                                // SAFETY: `NSData` guarantees `ptr` is valid for `len` bytes during callback.
                                unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
                            } else {
                                Vec::new()
                            }
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };
                    Ok((status, resp_bytes))
                };
                if let Ok(mut guard) = tx_for_block.lock()
                    && let Some(sender) = guard.take()
                {
                    let _ = sender.send(outcome);
                }
            },
        );

        // SAFETY: `NSURLSession.sharedSession` is thread-safe and returns a live `NSURLSession`.
        let session: Retained<AnyObject> =
            unsafe { msg_send![class!(NSURLSession), sharedSession] };
        // SAFETY: `dataTaskWithRequest:completionHandler:` retains the block and returns a task.
        let task: Retained<AnyObject> = unsafe {
            msg_send![
                &*session,
                dataTaskWithRequest: &*req,
                completionHandler: &*block
            ]
        };
        // SAFETY: `task` is a valid `NSURLSessionDataTask`.
        unsafe {
            let (): () = msg_send![&*task, resume];
        }
        Ok(task)
    })?;

    match rx.recv_timeout(timeout + Duration::from_secs(1)) {
        Ok(res) => res,
        Err(_) => {
            // SAFETY: `task` is a live `NSURLSessionDataTask`; `cancel` is thread-safe.
            unsafe {
                let (): () = msg_send![&*task, cancel];
            }
            Err("HTTPS request to proctor server timed out".to_string())
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn https_post(
    _url_str: &str,
    _content_type: &str,
    _room_code: Option<&str>,
    _join_token: Option<&str>,
    _body: &[u8],
    _timeout: Duration,
) -> Result<(i64, Vec<u8>), String> {
    Err("https:// proctor transport requires macOS NSURLSession".to_string())
}

/// Posts a single `StudentHeartbeat` over HTTP/1.1 or native macOS HTTPS to `server_url` (`/api/heartbeat`).
pub fn post_heartbeat(server_url: &Url, hb: &StudentHeartbeat) -> Result<HeartbeatResponse, String> {
    post_heartbeat_with_timeout(server_url, hb, Duration::from_secs(3))
}

/// Posts a single `StudentHeartbeat` with an explicit connect/read/write timeout.
pub fn post_heartbeat_with_timeout(
    server_url: &Url,
    hb: &StudentHeartbeat,
    timeout: Duration,
) -> Result<HeartbeatResponse, String> {
    let body = serde_json::to_vec(hb).map_err(|e| e.to_string())?;

    if server_url.scheme() == "https" {
        let endpoint = server_url
            .join("/api/heartbeat")
            .map_err(|e| e.to_string())?;
        let (status, resp_bytes) = https_post(
            endpoint.as_str(),
            "application/json",
            hb.room_code.as_deref(),
            hb.join_token.as_deref(),
            &body,
            timeout,
        )?;
        if !(200..300).contains(&status) {
            return Err(format!("proctor server returned HTTP {status}"));
        }
        return serde_json::from_slice::<HeartbeatResponse>(&resp_bytes).map_err(|e| e.to_string());
    }

    if server_url.scheme() != "http" {
        return Err(format!(
            "unsupported proctor server scheme '{}' (supported: http://, https://)",
            server_url.scheme()
        ));
    }
    let host = server_url
        .host_str()
        .ok_or_else(|| "proctor server_url missing host".to_string())?;
    let port = server_url.port().unwrap_or(80);
    let addr = format!("{host}:{port}");

    let sock_addr = addr
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| format!("unable to resolve proctor address '{addr}'"))?;
    let mut stream = TcpStream::connect_timeout(&sock_addr, timeout).map_err(|e| e.to_string())?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut req = format!(
        "POST /api/heartbeat HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(code) = hb.room_code.as_deref() {
        let clean = code.replace(['\r', '\n'], "");
        let _ = write!(req, "X-Room-Code: {clean}\r\n");
    }
    if let Some(token) = hb.join_token.as_deref() {
        let clean = token.replace(['\r', '\n'], "");
        let _ = write!(req, "X-Room-Token: {clean}\r\n");
    }
    req.push_str("\r\n");

    stream
        .write_all(req.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(&body).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut raw = Vec::new();
    stream
        .take(256 * 1024)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (_, resp_body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response from proctor server".to_string())?;

    serde_json::from_str::<HeartbeatResponse>(resp_body.trim()).map_err(|e| e.to_string())
}

/// Strips the `data:image/jpeg;base64,` prefix and decodes the payload into raw JPEG bytes.
pub fn decode_data_url_jpeg(data_url: &str) -> Option<Vec<u8>> {
    let b64 = data_url.strip_prefix("data:image/jpeg;base64,")?;
    let mut out = Vec::with_capacity((b64.len() / 4) * 3);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for &b in b64.as_bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn percent_encode_path_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn read_http_response(stream: &mut TcpStream) -> std::io::Result<bool> {
    let mut buf = [0u8; 1024];
    let mut len = 0usize;
    let header_end = loop {
        if len >= buf.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "response headers too large",
            ));
        }
        let n = stream.read(&mut buf[len..])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before response headers",
            ));
        }
        len += n;
        if let Some(pos) = buf[..len].windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };

    let headers = std::str::from_utf8(&buf[..header_end]).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "non-utf8 response headers")
    })?;
    let mut content_length = 0usize;
    let mut connection_close = false;
    for line in headers.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            if key.eq_ignore_ascii_case("content-length") {
                content_length = val.parse::<usize>().unwrap_or(0);
            } else if key.eq_ignore_ascii_case("connection") && val.eq_ignore_ascii_case("close") {
                connection_close = true;
            }
        }
    }

    let mut body_read = len - header_end;
    let mut discard = [0u8; 512];
    while body_read < content_length {
        let to_read = (content_length - body_read).min(discard.len());
        let n = stream.read(&mut discard[..to_read])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed before response body",
            ));
        }
        body_read += n;
    }

    Ok(!connection_close)
}

/// Persistent HTTP/1.1 keep-alive (or native macOS HTTPS) raw-JPEG uploader for high-FPS webcam and screen streaming.
pub struct WebcamUploader {
    addr: Option<String>,
    https_url: Option<String>,
    path: String,
    pub room_code: Option<String>,
    pub join_token: Option<String>,
    stream: Option<TcpStream>,
}

impl WebcamUploader {
    pub fn new(server_url: Url, student_id: String) -> Self {
        Self::with_endpoint(server_url, student_id, "/api/webcam-raw/")
    }

    pub fn with_endpoint(server_url: Url, student_id: String, endpoint_prefix: &str) -> Self {
        let encoded_id = percent_encode_path_segment(&student_id);
        let prefix = endpoint_prefix.trim_end_matches('/');
        let path = format!("{prefix}/{encoded_id}");
        let (addr, https_url) = match server_url.scheme() {
            "http" => (
                server_url
                    .host_str()
                    .map(|host| format!("{host}:{}", server_url.port().unwrap_or(80))),
                None,
            ),
            "https" => (None, server_url.join(&path).ok().map(|u| u.to_string())),
            _ => (None, None),
        };
        Self {
            addr,
            https_url,
            path,
            room_code: None,
            join_token: None,
            stream: None,
        }
    }

    pub fn with_room(mut self, room_code: Option<String>, join_token: Option<String>) -> Self {
        self.room_code = room_code;
        self.join_token = join_token;
        self
    }

    fn connect(addr: &str) -> Option<TcpStream> {
        let timeout = Duration::from_secs(1);
        let sock_addr = addr.to_socket_addrs().ok()?.next()?;
        let stream = TcpStream::connect_timeout(&sock_addr, timeout).ok()?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));
        Some(stream)
    }

    /// Uploads raw JPEG bytes over the persistent keep-alive socket (for `http://`)
    /// or native macOS `NSURLSession` (for `https://`).
    /// Drops the socket on any write/read failure so the next frame reconnects cleanly.
    pub fn send_raw_jpeg(&mut self, jpeg: &[u8]) -> bool {
        if jpeg.is_empty() {
            return false;
        }
        if let Some(url) = self.https_url.as_deref() {
            return matches!(
                https_post(
                    url,
                    "image/jpeg",
                    self.room_code.as_deref(),
                    self.join_token.as_deref(),
                    jpeg,
                    Duration::from_secs(2),
                ),
                Ok((status, _)) if (200..300).contains(&status)
            );
        }

        let Some(addr) = self.addr.as_deref() else {
            return false;
        };

        if self.stream.is_none() {
            self.stream = Self::connect(addr);
        }
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };

        let mut header = format!(
            "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nConnection: keep-alive\r\n",
            self.path,
            jpeg.len()
        );
        if let Some(code) = self.room_code.as_deref() {
            let clean = code.replace(['\r', '\n'], "");
            let _ = write!(header, "X-Room-Code: {clean}\r\n");
        }
        if let Some(token) = self.join_token.as_deref() {
            let clean = token.replace(['\r', '\n'], "");
            let _ = write!(header, "X-Room-Token: {clean}\r\n");
        }
        header.push_str("\r\n");

        let io_result = stream
            .write_all(header.as_bytes())
            .and_then(|()| stream.write_all(jpeg))
            .and_then(|()| stream.flush())
            .and_then(|()| read_http_response(stream));

        match io_result {
            Ok(keep_alive) => {
                if !keep_alive {
                    self.stream = None;
                }
                true
            }
            Err(_) => {
                self.stream = None;
                false
            }
        }
    }

    /// Decodes `data_url` to raw JPEG bytes and uploads it over the persistent keep-alive socket.
    /// Drops the socket on any write/read failure so the next frame reconnects cleanly.
    pub fn send_frame(&mut self, data_url: &str) -> bool {
        let Some(jpeg_bytes) = decode_data_url_jpeg(data_url) else {
            return false;
        };
        self.send_raw_jpeg(&jpeg_bytes)
    }
}

pub struct HeartbeatConfig {
    pub server_url: Url,
    pub room_code: Option<String>,
    pub join_token: Option<String>,
    pub student_id: String,
    pub exam_name: String,
    pub config_key: String,
    pub interval_secs: u64,
}

pub fn spawn_heartbeat_thread<F, P>(
    cfg: HeartbeatConfig,
    telemetry: Arc<Mutex<TelemetryState>>,
    stop_flag: Arc<AtomicBool>,
    on_force_unlock: F,
    on_policy_update: P,
) where
    F: FnOnce() + Send + 'static,
    P: Fn(RoomPolicyOverrides) + Send + 'static,
{
    let interval = Duration::from_secs(cfg.interval_secs.clamp(1, 2));
    std::thread::spawn(move || {
        let mut on_unlock = Some(on_force_unlock);
        let mut last_policy: Option<RoomPolicyOverrides> = None;
        while !stop_flag.load(Ordering::Relaxed) {
            let snap = telemetry.lock().unwrap().clone();
            let hb = StudentHeartbeat {
                student_id: cfg.student_id.clone(),
                room_code: cfg.room_code.clone(),
                join_token: cfg.join_token.clone(),
                exam_name: cfg.exam_name.clone(),
                config_key: cfg.config_key.clone(),
                status: snap.status,
                active_host: snap.active_host,
                violations: snap.violations,
            };
            if let Ok(resp) = post_heartbeat(&cfg.server_url, &hb) {
                if last_policy.as_ref() != Some(&resp.policy) {
                    last_policy = Some(resp.policy.clone());
                    on_policy_update(resp.policy);
                }
                if resp.command == ProctorCommand::ForceUnlock {
                    if let Some(cb) = on_unlock.take() {
                        cb();
                    }
                    break;
                }
            }
            std::thread::sleep(interval);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn decode_data_url_jpeg_decodes_valid_and_rejects_invalid() {
        // "Man" -> TWFu, "Ma" -> TWE=, "M" -> TQ==
        assert_eq!(
            decode_data_url_jpeg("data:image/jpeg;base64,TWFu").as_deref(),
            Some(&b"Man"[..])
        );
        assert_eq!(
            decode_data_url_jpeg("data:image/jpeg;base64,TWE=").as_deref(),
            Some(&b"Ma"[..])
        );
        assert_eq!(
            decode_data_url_jpeg("data:image/jpeg;base64,TQ==").as_deref(),
            Some(&b"M"[..])
        );
        // Sample JPEG SOI/EOI bytes: FF D8 FF E0 -> /9j/4A==
        assert_eq!(
            decode_data_url_jpeg("data:image/jpeg;base64,/9j/4A==").as_deref(),
            Some(&[0xFF, 0xD8, 0xFF, 0xE0][..])
        );
        assert!(decode_data_url_jpeg("data:image/png;base64,TWFu").is_none());
        assert!(decode_data_url_jpeg("data:image/jpeg;base64,!!!").is_none());
        assert!(decode_data_url_jpeg("data:image/jpeg;base64,").is_none());
    }

    #[test]
    fn webcam_uploader_reuses_keep_alive_socket_and_reconnects_after_drop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let read_req = |reader: &mut std::io::BufReader<&mut TcpStream>| -> (String, Vec<u8>) {
            use std::io::BufRead;
            let mut headers = String::new();
            let mut content_len = 0usize;
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).unwrap();
                assert!(n > 0);
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':')
                    && k.trim().eq_ignore_ascii_case("content-length")
                {
                    content_len = v.trim().parse().unwrap_or(0);
                }
                headers.push_str(&line);
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).unwrap();
            (headers, body)
        };

        let server = std::thread::spawn(move || {
            // Connection 1: serve two frames on the same keep-alive socket, then close.
            let (mut s1, _) = listener.accept().unwrap();
            let mut frames = Vec::new();
            for _ in 0..2 {
                let (req, body) = {
                    let mut reader = std::io::BufReader::new(&mut s1);
                    read_req(&mut reader)
                };
                assert!(req.starts_with("POST /api/webcam-raw/alice%20smith HTTP/1.1\r\n"));
                assert!(req.contains("Content-Type: image/jpeg\r\n"));
                assert!(req.contains("Connection: keep-alive\r\n"));
                frames.push(body);
                s1.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                )
                .unwrap();
                s1.flush().unwrap();
            }
            drop(s1);

            // Connection 2: verify uploader reconnects on the next frame.
            let (mut s2, _) = listener.accept().unwrap();
            let (_req, body) = {
                let mut reader = std::io::BufReader::new(&mut s2);
                read_req(&mut reader)
            };
            frames.push(body);
            s2.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
            )
            .unwrap();
            s2.flush().unwrap();
            frames
        });

        let url = Url::parse(&format!("http://{addr}")).unwrap();
        let mut uploader = WebcamUploader::new(url, "alice smith".to_string());

        assert!(uploader.send_frame("data:image/jpeg;base64,/9j/4A=="));
        assert!(uploader.send_frame("data:image/jpeg;base64,TWFu"));
        // Server closed s1; this send hits EOF and drops the socket cleanly,
        // and the next frame reconnects on s2.
        assert!(!uploader.send_frame("data:image/jpeg;base64,TWE="));
        assert!(uploader.send_frame("data:image/jpeg;base64,TQ=="));

        let received = server.join().unwrap();
        assert_eq!(received.len(), 3);
        assert_eq!(received[0], vec![0xFF, 0xD8, 0xFF, 0xE0]);
        assert_eq!(received[1], b"Man".to_vec());
        assert_eq!(received[2], b"M".to_vec());
    }

    #[test]
    fn screen_uploader_with_endpoint_posts_raw_jpeg_over_keep_alive() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            use std::io::BufRead;
            let (mut stream, _) = listener.accept().unwrap();
            let mut frames = Vec::new();
            for _ in 0..2 {
                let (req, body) = {
                    let mut reader = std::io::BufReader::new(&mut stream);
                    let mut headers = String::new();
                    let mut content_len = 0usize;
                    loop {
                        let mut line = String::new();
                        let n = reader.read_line(&mut line).unwrap();
                        assert!(n > 0);
                        if line == "\r\n" || line == "\n" {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':')
                            && k.trim().eq_ignore_ascii_case("content-length")
                        {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                        headers.push_str(&line);
                    }
                    let mut body = vec![0u8; content_len];
                    reader.read_exact(&mut body).unwrap();
                    (headers, body)
                };
                assert!(req.starts_with("POST /api/screen-raw/student%2042 HTTP/1.1\r\n"));
                assert!(req.contains("Content-Type: image/jpeg\r\n"));
                assert!(req.contains("Connection: keep-alive\r\n"));
                frames.push(body);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                    )
                    .unwrap();
                stream.flush().unwrap();
            }
            frames
        });

        let url = Url::parse(&format!("http://{addr}")).unwrap();
        let mut uploader =
            WebcamUploader::with_endpoint(url, "student 42".to_string(), "/api/screen-raw/");

        let frame1 = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let frame2 = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x01, 0x02, 0xFF, 0xD9];
        assert!(uploader.send_raw_jpeg(&frame1));
        assert!(uploader.send_raw_jpeg(&frame2));

        let received = server.join().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0], frame1);
        assert_eq!(received[1], frame2);
    }

    #[test]
    fn webcam_uploader_sends_room_code_and_join_token_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            use std::io::BufRead;
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut headers = String::new();
            let mut content_len = 0usize;
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).unwrap();
                assert!(n > 0);
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':')
                    && k.trim().eq_ignore_ascii_case("content-length")
                {
                    content_len = v.trim().parse().unwrap_or(0);
                }
                headers.push_str(&line);
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).unwrap();
            drop(reader);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                )
                .unwrap();
            stream.flush().unwrap();
            (headers, body)
        });

        let url = Url::parse(&format!("http://{addr}")).unwrap();
        let mut uploader = WebcamUploader::new(url, "alice".to_string()).with_room(
            Some("AP-BIO-101".to_string()),
            Some("tok-secret-999".to_string()),
        );
        let frame = vec![0xFF, 0xD8, 0xFF, 0xD9];
        assert!(uploader.send_raw_jpeg(&frame));

        let (headers, body) = server.join().unwrap();
        assert!(headers.starts_with("POST /api/webcam-raw/alice HTTP/1.1\r\n"));
        assert!(headers.contains("X-Room-Code: AP-BIO-101\r\n"));
        assert!(headers.contains("X-Room-Token: tok-secret-999\r\n"));
        assert_eq!(body, frame);
    }

    #[test]
    fn post_heartbeat_sends_room_code_and_join_token_headers_and_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = std::thread::spawn(move || {
            use std::io::BufRead;
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(&mut stream);
            let mut headers = String::new();
            let mut content_len = 0usize;
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).unwrap();
                assert!(n > 0);
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':')
                    && k.trim().eq_ignore_ascii_case("content-length")
                {
                    content_len = v.trim().parse().unwrap_or(0);
                }
                headers.push_str(&line);
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).unwrap();
            drop(reader);

            let resp = HeartbeatResponse {
                command: ProctorCommand::None,
                config_key_verified: true,
                policy: RoomPolicyOverrides::default(),
            };
            let resp_json = serde_json::to_string(&resp).unwrap();
            let http_resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_json.len(),
                resp_json
            );
            stream.write_all(http_resp.as_bytes()).unwrap();
            stream.flush().unwrap();
            (headers, body)
        });

        let url = Url::parse(&format!("http://{addr}")).unwrap();
        let hb = StudentHeartbeat {
            student_id: "alice".to_string(),
            room_code: Some("CS-202".to_string()),
            join_token: Some("join-tok-abc".to_string()),
            exam_name: "Midterm".to_string(),
            config_key: "deadbeef".to_string(),
            status: StudentStatus::InExam,
            active_host: "exam.example.edu".to_string(),
            violations: Vec::new(),
        };
        let resp = post_heartbeat_with_timeout(&url, &hb, Duration::from_secs(2)).unwrap();
        assert!(resp.config_key_verified);
        assert_eq!(resp.command, ProctorCommand::None);

        let (headers, body) = server.join().unwrap();
        assert!(headers.starts_with("POST /api/heartbeat HTTP/1.1\r\n"));
        assert!(headers.contains("X-Room-Code: CS-202\r\n"));
        assert!(headers.contains("X-Room-Token: join-tok-abc\r\n"));
        let decoded: StudentHeartbeat = serde_json::from_slice(&body).unwrap();
        assert_eq!(decoded.room_code.as_deref(), Some("CS-202"));
        assert_eq!(decoded.join_token.as_deref(), Some("join-tok-abc"));
    }
}
