mod dashboard;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use rustbrowser_core::{
    ExamConfig, HeartbeatResponse, ProctorCommand, RoomPolicyOverrides, RoomStateSnapshot,
    RoomStudentView, StudentHeartbeat, StudentStatus, WebcamFrameUpload, hash_password,
    parse_seb_file,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(
    name = "rld-proctor",
    about = "RLD Proctor — live classroom dashboard, webcam & screen monitor for RLD Browser"
)]
struct Cli {
    /// Address and port to bind the HTTP proctor server on (0.0.0.0 allows teacher devices on LAN)
    #[arg(long, default_value = "0.0.0.0:8443")]
    bind: String,

    /// Classroom room code label
    #[arg(long, default_value = "AP-CS-P2")]
    room: String,

    /// Optional exam.toml or exam.seb file to pin the expected Config Key
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Optional proctor admin auth token (defaults to a random 128-bit hex token per session)
    #[arg(long)]
    token: Option<String>,
}

#[derive(Debug, Default)]
struct WebcamChannel {
    seq: u64,
    jpeg: Option<Vec<u8>>,
    closed: bool,
}

type SharedWebcamChannel = Arc<(Mutex<WebcamChannel>, Condvar)>;

fn new_webcam_channel() -> SharedWebcamChannel {
    Arc::new((Mutex::new(WebcamChannel::default()), Condvar::new()))
}

#[derive(Debug)]
struct StudentRecord {
    exam_name: String,
    config_key: String,
    config_key_verified: bool,
    status: StudentStatus,
    active_host: String,
    violations: Vec<String>,
    last_seen: Instant,
    force_unlock: bool,
    webcam_data_url: Option<String>,
    webcam_channel: SharedWebcamChannel,
    screen_channel: SharedWebcamChannel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CredentialsSnapshot {
    pub entrance_code: Option<String>,
    pub quit_password: Option<String>,
}

#[derive(Debug)]
pub struct RoomStore {
    room_code: String,
    expected_config_key: Option<String>,
    policy: RoomPolicyOverrides,
    entrance_code_plain: Option<String>,
    quit_password_plain: Option<String>,
    students: BTreeMap<String, StudentRecord>,
}

impl RoomStore {
    pub fn new(room_code: String, expected_config_key: Option<String>) -> Self {
        Self {
            room_code,
            expected_config_key,
            policy: RoomPolicyOverrides {
                allowed_domains: vec!["example.com".to_string(), "*.iana.org".to_string()],
                require_webcam: true,
                allow_read_aloud: true,
                ..RoomPolicyOverrides::default()
            },
            entrance_code_plain: None,
            quit_password_plain: None,
            students: BTreeMap::new(),
        }
    }

    pub fn credentials(&self) -> CredentialsSnapshot {
        CredentialsSnapshot {
            entrance_code: self.entrance_code_plain.clone(),
            quit_password: self.quit_password_plain.clone(),
        }
    }

    pub fn policy_from_config(cfg: &ExamConfig) -> RoomPolicyOverrides {
        RoomPolicyOverrides {
            allowed_domains: cfg.allowed_domains.clone(),
            blocked_domains: cfg.blocked_domains.clone(),
            pinned_tabs: cfg.pinned_tabs.clone(),
            entrance_code_hash: cfg.entrance_code_hash.clone(),
            quit_password_hash: cfg.quit_password_hash.clone(),
            require_webcam: cfg.proctor.enable_webcam,
            block_clipboard: cfg.lockdown.block_clipboard,
            allow_read_aloud: cfg.accessibility.allow_read_aloud,
            allow_find_in_page: cfg.browser.allow_find_in_page,
            allow_back_forward: cfg.lockdown.allow_back_forward,
            allow_reload: cfg.lockdown.allow_reload,
            allow_zoom: cfg.browser.allow_zoom,
            block_downloads: cfg.browser.block_downloads,
            block_multiple_displays: cfg.security.block_multiple_displays,
            block_screen_capture_apps: cfg.security.block_screen_capture_apps,
        }
    }

    pub fn set_policy(&mut self, policy: RoomPolicyOverrides) {
        self.policy = policy;
    }

    pub fn ingest_heartbeat(&mut self, mut hb: StudentHeartbeat) -> HeartbeatResponse {
        hb.student_id = sanitize_student_id(&hb.student_id);
        hb.violations = sanitize_violations(hb.violations);

        let verified = match &self.expected_config_key {
            Some(expected) => expected == &hb.config_key,
            None => true,
        };

        if !self.students.contains_key(&hb.student_id) && self.students.len() >= MAX_STUDENTS {
            return HeartbeatResponse {
                command: ProctorCommand::None,
                config_key_verified: verified,
                policy: self.policy.clone(),
            };
        }

        let entry = self
            .students
            .entry(hb.student_id)
            .or_insert_with(|| StudentRecord {
                exam_name: hb.exam_name.clone(),
                config_key: hb.config_key.clone(),
                config_key_verified: verified,
                status: hb.status,
                active_host: hb.active_host.clone(),
                violations: hb.violations.clone(),
                last_seen: Instant::now(),
                force_unlock: false,
                webcam_data_url: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });

        entry.exam_name = hb.exam_name;
        entry.config_key = hb.config_key;
        entry.config_key_verified = verified;
        entry.status = hb.status;
        entry.active_host = hb.active_host;
        entry.violations = hb.violations;
        entry.last_seen = Instant::now();

        let command = if entry.force_unlock {
            // One-shot delivery: clear the flag so the student can reopen the app later.
            entry.force_unlock = false;
            entry.status = StudentStatus::Exited;
            ProctorCommand::ForceUnlock
        } else {
            ProctorCommand::None
        };

        if entry.status == StudentStatus::Exited {
            entry.webcam_data_url = None;
            {
                let mut ch = entry
                    .webcam_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                ch.jpeg = None;
                ch.closed = true;
                entry.webcam_channel.1.notify_all();
            }
            {
                let mut ch = entry
                    .screen_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                ch.jpeg = None;
                ch.closed = true;
                entry.screen_channel.1.notify_all();
            }
        } else {
            entry
                .webcam_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed = false;
            entry
                .screen_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .closed = false;
        }

        HeartbeatResponse {
            command,
            config_key_verified: verified,
            policy: self.policy.clone(),
        }
    }

    pub fn ingest_webcam(&mut self, mut upload: WebcamFrameUpload) -> bool {
        upload.student_id = sanitize_student_id(&upload.student_id);
        if !upload.data_url.starts_with("data:image/jpeg;base64,")
            && !upload.data_url.starts_with("data:image/webp;base64,")
        {
            return false;
        }
        if !self.students.contains_key(&upload.student_id) && self.students.len() >= MAX_STUDENTS {
            return false;
        }
        if self
            .students
            .get(&upload.student_id)
            .is_some_and(|rec| rec.status == StudentStatus::Exited)
        {
            return false;
        }
        let decoded_jpeg = decode_base64_jpeg(&upload.data_url);
        let entry = self
            .students
            .entry(upload.student_id)
            .or_insert_with(|| StudentRecord {
                exam_name: "Exam Session".to_string(),
                config_key: String::new(),
                config_key_verified: self.expected_config_key.is_none(),
                status: StudentStatus::InExam,
                active_host: String::new(),
                violations: Vec::new(),
                last_seen: Instant::now(),
                force_unlock: false,
                webcam_data_url: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });
        entry.webcam_data_url = Some(upload.data_url);
        if let Some(jpeg_bytes) = decoded_jpeg {
            let mut ch = entry
                .webcam_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ch.seq = ch.seq.wrapping_add(1).max(1);
            ch.jpeg = Some(jpeg_bytes);
            ch.closed = false;
            entry.webcam_channel.1.notify_all();
        }
        entry.last_seen = Instant::now();
        true
    }

    pub fn ingest_webcam_raw(&mut self, student_id: String, jpeg_bytes: Vec<u8>) -> bool {
        let student_id = sanitize_student_id(&student_id);
        if jpeg_bytes.is_empty() {
            return false;
        }
        if !self.students.contains_key(&student_id) && self.students.len() >= MAX_STUDENTS {
            return false;
        }
        if self
            .students
            .get(&student_id)
            .is_some_and(|rec| rec.status == StudentStatus::Exited)
        {
            return false;
        }
        let entry = self
            .students
            .entry(student_id)
            .or_insert_with(|| StudentRecord {
                exam_name: "Exam Session".to_string(),
                config_key: String::new(),
                config_key_verified: self.expected_config_key.is_none(),
                status: StudentStatus::InExam,
                active_host: String::new(),
                violations: Vec::new(),
                last_seen: Instant::now(),
                force_unlock: false,
                webcam_data_url: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });
        {
            let mut ch = entry
                .webcam_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ch.seq = ch.seq.wrapping_add(1).max(1);
            ch.jpeg = Some(jpeg_bytes);
            ch.closed = false;
            entry.webcam_channel.1.notify_all();
        }
        entry.last_seen = Instant::now();
        true
    }

    fn webcam_channel(&self, student_id: &str) -> Option<SharedWebcamChannel> {
        self.students
            .get(student_id)
            .map(|r| Arc::clone(&r.webcam_channel))
    }

    pub fn ingest_screen_raw(&mut self, student_id: &str, jpeg: Vec<u8>) -> bool {
        let student_id = sanitize_student_id(student_id);
        if jpeg.is_empty() {
            return false;
        }
        if !self.students.contains_key(&student_id) && self.students.len() >= MAX_STUDENTS {
            return false;
        }
        if self
            .students
            .get(&student_id)
            .is_some_and(|rec| rec.status == StudentStatus::Exited)
        {
            return false;
        }
        let entry = self
            .students
            .entry(student_id)
            .or_insert_with(|| StudentRecord {
                exam_name: "Exam Session".to_string(),
                config_key: String::new(),
                config_key_verified: self.expected_config_key.is_none(),
                status: StudentStatus::InExam,
                active_host: String::new(),
                violations: Vec::new(),
                last_seen: Instant::now(),
                force_unlock: false,
                webcam_data_url: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });
        {
            let mut ch = entry
                .screen_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ch.seq = ch.seq.wrapping_add(1).max(1);
            ch.jpeg = Some(jpeg);
            ch.closed = false;
            entry.screen_channel.1.notify_all();
        }
        entry.last_seen = Instant::now();
        true
    }

    fn screen_channel(&self, student_id: &str) -> Option<SharedWebcamChannel> {
        self.students
            .get(student_id)
            .map(|r| Arc::clone(&r.screen_channel))
    }

    pub fn get_webcam(&self, student_id: &str) -> Option<&str> {
        self.students
            .get(student_id)
            .and_then(|r| r.webcam_data_url.as_deref())
    }

    pub fn unlock_student(&mut self, student_id: &str) -> bool {
        if let Some(rec) = self.students.get_mut(student_id) {
            rec.force_unlock = true;
            true
        } else {
            false
        }
    }

    pub fn unlock_all(&mut self) {
        for rec in self.students.values_mut() {
            if rec.status != StudentStatus::Exited {
                rec.force_unlock = true;
            }
        }
    }

    pub fn snapshot(&self) -> RoomStateSnapshot {
        let now = Instant::now();
        let mut in_exam_count = 0usize;
        let mut flagged_count = 0usize;
        let mut students = Vec::with_capacity(self.students.len());

        for (id, rec) in &self.students {
            if rec.status == StudentStatus::InExam {
                in_exam_count += 1;
            }
            if !rec.config_key_verified
                || !rec.violations.is_empty()
                || rec.status == StudentStatus::SecurityPaused
            {
                flagged_count += 1;
            }
            students.push(RoomStudentView {
                student_id: id.clone(),
                exam_name: rec.exam_name.clone(),
                config_key: rec.config_key.clone(),
                config_key_verified: rec.config_key_verified,
                status: rec.status,
                active_host: rec.active_host.clone(),
                violations: rec.violations.clone(),
                last_seen_secs_ago: now.saturating_duration_since(rec.last_seen).as_secs(),
                force_unlock_pending: rec.force_unlock,
                has_webcam: rec.webcam_data_url.is_some()
                    || rec
                        .webcam_channel
                        .0
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .jpeg
                        .is_some(),
                has_screen: rec
                    .screen_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .jpeg
                    .is_some(),
            });
        }

        RoomStateSnapshot {
            room_code: self.room_code.clone(),
            expected_config_key: self.expected_config_key.clone(),
            total_students: students.len(),
            in_exam_count,
            flagged_count,
            policy: self.policy.clone(),
            students,
        }
    }
}

#[derive(Debug, Deserialize)]
struct UnlockRequest {
    student_id: String,
}

#[derive(Debug, Deserialize)]
struct CredentialUpdateRequest {
    kind: String,
    value: Option<String>,
}

fn detect_lan_ip() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let addr = sock.local_addr().ok()?;
    Some(addr.ip().to_string())
}

const MAX_REQUEST_BODY_BYTES: usize = 512 * 1024;
const MAX_SCREEN_BODY_BYTES: usize = 1024 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 4096;
const MAX_HEADERS: usize = 64;
const MAX_CONCURRENT_CONNECTIONS: usize = 128;
const CONNECTION_IO_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_STUDENT_ID_CHARS: usize = 64;
const MAX_VIOLATIONS: usize = 32;
const MAX_VIOLATION_CHARS: usize = 256;
const MAX_STUDENTS: usize = 256;

const UNAUTHORIZED_HTML: &[u8] = br#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8" /><title>RLD Proctor &mdash; Authentication Required</title></head>
<body style="font-family:-apple-system,system-ui,sans-serif;max-width:480px;margin:80px auto;padding:0 20px;color:#192230;">
<h1>RLD Proctor &mdash; Authentication Required</h1>
<p>Open the tokenized dashboard URL printed in the <code>rld-proctor</code> terminal to authenticate this browser session.</p>
</body>
</html>"#;

fn truncate_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn sanitize_student_id(id: &str) -> String {
    truncate_chars(id, MAX_STUDENT_ID_CHARS)
}

fn sanitize_violations(violations: Vec<String>) -> Vec<String> {
    violations
        .into_iter()
        .take(MAX_VIOLATIONS)
        .map(|v| truncate_chars(&v, MAX_VIOLATION_CHARS))
        .collect()
}

fn generate_proctor_token() -> String {
    let mut bytes = [0u8; 16];
    let filled_from_urandom = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !filled_from_urandom {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = u128::from(std::process::id());
        let mixed = nanos ^ pid.wrapping_mul(0x9e37_79b9_7f4a_7c15_u128).rotate_left(37);
        bytes = mixed.to_le_bytes();
    }
    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn extract_query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=')
            && k == key
        {
            return Some(percent_decode(v));
        }
    }
    None
}

fn cookie_has_proctor_token(cookie_header: &str, expected: &str) -> bool {
    for part in cookie_header.split(';') {
        if let Some((name, val)) = part.trim().split_once('=')
            && name.trim() == "rb_proctor"
            && constant_time_eq(val.trim().as_bytes(), expected.as_bytes())
        {
            return true;
        }
    }
    false
}

struct ConnectionGuard {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn try_acquire_connection(active: &Arc<AtomicUsize>) -> Option<ConnectionGuard> {
    active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |curr| {
            (curr < MAX_CONCURRENT_CONNECTIONS).then_some(curr + 1)
        })
        .ok()
        .map(|_| ConnectionGuard {
            active: Arc::clone(active),
        })
}

enum LineReadError {
    Io(std::io::Error),
    TooLong,
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<String>, LineReadError> {
    let mut out = Vec::new();
    loop {
        let buf = reader.fill_buf().map_err(LineReadError::Io)?;
        if buf.is_empty() {
            return if out.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8_lossy(&out).into_owned()))
            };
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let take = pos + 1;
            if out.len() + take > max_bytes {
                return Err(LineReadError::TooLong);
            }
            out.extend_from_slice(&buf[..take]);
            reader.consume(take);
            return Ok(Some(String::from_utf8_lossy(&out).into_owned()));
        }
        let take = buf.len();
        if out.len() + take > max_bytes {
            return Err(LineReadError::TooLong);
        }
        out.extend_from_slice(buf);
        reader.consume(take);
    }
}

fn write_unauthorized_api(stream: &mut TcpStream) -> std::io::Result<()> {
    write_response(
        stream,
        "401 Unauthorized",
        "application/json",
        br#"{"error":"proctor authentication required"}"#,
    )
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let loaded_config = match &cli.config {
        Some(path) => {
            let is_seb = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("seb"));
            if is_seb {
                match parse_seb_file(path) {
                    Ok(doc) => Some(doc.config),
                    Err(err) => {
                        eprintln!("error: failed to load .seb config: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            } else {
                match ExamConfig::from_toml_file(path) {
                    Ok(cfg) => Some(cfg),
                    Err(err) => {
                        eprintln!("error: failed to load config: {err}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
        None => None,
    };
    let expected_key = loaded_config.as_ref().map(|c| c.config_key());

    let listener = match TcpListener::bind(&cli.bind) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("error: failed to bind {}: {err}", cli.bind);
            return ExitCode::FAILURE;
        }
    };

    let proctor_token = cli
        .token
        .filter(|t| !t.is_empty())
        .unwrap_or_else(generate_proctor_token);

    let port = listener.local_addr().map(|a| a.port()).unwrap_or(8443);
    println!(
        "[rld-proctor] Room '{}' ready (pinned_key={})",
        cli.room,
        expected_key
            .as_deref()
            .map(|k| &k[..12])
            .unwrap_or("unpinned")
    );
    println!("  Local dashboard:   http://127.0.0.1:{port}/?token={proctor_token}");
    if let Some(lan_ip) = detect_lan_ip() {
        println!("  Teacher LAN URL:   http://{lan_ip}:{port}/?token={proctor_token}");
    }

    let mut initial_store = RoomStore::new(cli.room, expected_key);
    if let Some(cfg) = &loaded_config {
        initial_store.set_policy(RoomStore::policy_from_config(cfg));
    }
    let store = Arc::new(Mutex::new(initial_store));
    let token = Arc::new(proctor_token);
    let active_connections = Arc::new(AtomicUsize::new(0));

    for mut stream in listener.incoming().flatten() {
        let Some(guard) = try_acquire_connection(&active_connections) else {
            let _ = stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT));
            let _ = write_response(
                &mut stream,
                "503 Service Unavailable",
                "text/plain; charset=utf-8",
                b"Service Unavailable",
            );
            continue;
        };
        let store = Arc::clone(&store);
        let token = Arc::clone(&token);
        std::thread::spawn(move || {
            let _guard = guard;
            let _ = handle_connection(stream, &store, &token);
        });
    }

    ExitCode::SUCCESS
}

fn handle_connection(
    mut stream: TcpStream,
    store: &Arc<Mutex<RoomStore>>,
    proctor_token: &str,
) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(CONNECTION_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT));
    let mut reader = BufReader::new(stream.try_clone()?);

    loop {
        let request_line = match read_bounded_line(&mut reader, MAX_HEADER_LINE_BYTES) {
            Ok(Some(line)) => line,
            Ok(None) => return Ok(()),
            Err(LineReadError::Io(e)) => return Err(e),
            Err(LineReadError::TooLong) => {
                return write_response(
                    &mut stream,
                    "431 Request Header Fields Too Large",
                    "text/plain; charset=utf-8",
                    b"Request Header Fields Too Large",
                );
            }
        };
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let raw_path = parts.next().unwrap_or("");
        if method.is_empty() || raw_path.is_empty() {
            return write_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                b"Bad Request",
            );
        }
        let (path, query) = raw_path
            .split_once('?')
            .map_or((raw_path, None), |(p, q)| (p, Some(q)));
        let max_body_bytes = if method == "POST" && path.starts_with("/api/screen-raw/") {
            MAX_SCREEN_BODY_BYTES
        } else {
            MAX_REQUEST_BODY_BYTES
        };

        let mut content_length = 0usize;
        let mut connection_close = false;
        let mut body_too_large = false;
        let mut authenticated = false;
        let mut header_count = 0usize;
        loop {
            let line = match read_bounded_line(&mut reader, MAX_HEADER_LINE_BYTES) {
                Ok(Some(l)) => l,
                Ok(None) => break,
                Err(LineReadError::Io(e)) => return Err(e),
                Err(LineReadError::TooLong) => {
                    return write_response(
                        &mut stream,
                        "431 Request Header Fields Too Large",
                        "text/plain; charset=utf-8",
                        b"Request Header Fields Too Large",
                    );
                }
            };
            if line == "\r\n" || line == "\n" {
                break;
            }
            header_count += 1;
            if header_count > MAX_HEADERS {
                return write_response(
                    &mut stream,
                    "431 Request Header Fields Too Large",
                    "text/plain; charset=utf-8",
                    b"Request Header Fields Too Large",
                );
            }
            if let Some((name, val)) = line.split_once(':') {
                let name = name.trim();
                let val = val.trim();
                if name.eq_ignore_ascii_case("content-length") {
                    if let Ok(len) = val.parse::<usize>() {
                        if len > max_body_bytes {
                            body_too_large = true;
                        } else {
                            content_length = len;
                        }
                    }
                } else if name.eq_ignore_ascii_case("connection")
                    && val.to_ascii_lowercase().contains("close")
                {
                    connection_close = true;
                } else if (name.eq_ignore_ascii_case("x-proctor-token")
                    && constant_time_eq(val.as_bytes(), proctor_token.as_bytes()))
                    || (name.eq_ignore_ascii_case("cookie")
                        && cookie_has_proctor_token(val, proctor_token))
                {
                    authenticated = true;
                }
            }
        }

        if body_too_large {
            return write_response(
                &mut stream,
                "413 Payload Too Large",
                "text/plain; charset=utf-8",
                b"Payload Too Large",
            );
        }

        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body)?;
        }

        if method == "GET"
            && path == "/"
            && let Some(q) = query
            && let Some(tok) = extract_query_param(q, "token")
            && constant_time_eq(tok.as_bytes(), proctor_token.as_bytes())
        {
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: /\r\nSet-Cookie: rb_proctor={proctor_token}; Path=/; HttpOnly; SameSite=Strict\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(resp.as_bytes())?;
            return stream.flush();
        }

        if method == "POST"
            && let Some(student_id) = path.strip_prefix("/api/webcam-raw/")
        {
            let decoded_id = percent_decode(student_id);
            store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ingest_webcam_raw(decoded_id, body);
            if connection_close {
                stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )?;
                return stream.flush();
            }
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
            )?;
            stream.flush()?;
            continue;
        }

        if method == "GET"
            && let Some(student_id) = path.strip_prefix("/api/webcam-stream/")
        {
            if !authenticated {
                return write_unauthorized_api(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let channel = store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .webcam_channel(&decoded_id);
            return match channel {
                Some(ch) => serve_mjpeg_stream(&mut stream, ch),
                None => write_response(
                    &mut stream,
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    b"Not Found",
                ),
            };
        }

        if method == "POST"
            && let Some(student_id) = path.strip_prefix("/api/screen-raw/")
        {
            let decoded_id = percent_decode(student_id);
            store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .ingest_screen_raw(&decoded_id, body);
            if connection_close {
                stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                )?;
                return stream.flush();
            }
            stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
            )?;
            stream.flush()?;
            continue;
        }

        if method == "GET"
            && let Some(student_id) = path.strip_prefix("/api/screen-stream/")
        {
            if !authenticated {
                return write_unauthorized_api(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let channel = store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .screen_channel(&decoded_id);
            return match channel {
                Some(ch) => serve_mjpeg_stream(&mut stream, ch),
                None => write_response(
                    &mut stream,
                    "404 Not Found",
                    "text/plain; charset=utf-8",
                    b"Not Found",
                ),
            };
        }

        if method == "GET"
            && let Some(student_id) = path.strip_prefix("/api/webcam/")
        {
            if !authenticated {
                return write_unauthorized_api(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let frame = store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_webcam(&decoded_id)
                .map(str::to_string);
            let payload = serde_json::json!({ "data_url": frame }).to_string();
            return write_response(
                &mut stream,
                "200 OK",
                "application/json",
                payload.as_bytes(),
            );
        }

        return match (method, path) {
            ("GET", "/") => {
                if !authenticated {
                    return write_response(
                        &mut stream,
                        "401 Unauthorized",
                        "text/html; charset=utf-8",
                        UNAUTHORIZED_HTML,
                    );
                }
                write_response(
                    &mut stream,
                    "200 OK",
                    "text/html; charset=utf-8",
                    dashboard::dashboard_html().as_bytes(),
                )
            }
            ("GET", "/api/state") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                let snap = store.lock().unwrap_or_else(|e| e.into_inner()).snapshot();
                let json = serde_json::to_vec(&snap).unwrap_or_default();
                write_response(&mut stream, "200 OK", "application/json", &json)
            }
            ("GET", "/api/credentials") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                let creds = store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .credentials();
                let json = serde_json::to_vec(&creds).unwrap_or_default();
                write_response(&mut stream, "200 OK", "application/json", &json)
            }
            ("POST", "/api/heartbeat") => match serde_json::from_slice::<StudentHeartbeat>(&body) {
                Ok(hb) => {
                    let resp = store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .ingest_heartbeat(hb);
                    let json = serde_json::to_vec(&resp).unwrap_or_default();
                    write_response(&mut stream, "200 OK", "application/json", &json)
                }
                Err(_) => write_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    br#"{"error":"invalid heartbeat payload"}"#,
                ),
            },
            ("POST", "/api/webcam") => match serde_json::from_slice::<WebcamFrameUpload>(&body) {
                Ok(upload) => {
                    let ok = store
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .ingest_webcam(upload);
                    let payload = if ok {
                        br#"{"ok":true}"#.as_slice()
                    } else {
                        br#"{"ok":false}"#.as_slice()
                    };
                    write_response(&mut stream, "200 OK", "application/json", payload)
                }
                Err(_) => write_response(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    br#"{"error":"invalid webcam payload"}"#,
                ),
            },
            ("POST", "/api/unlock-all") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                store.lock().unwrap_or_else(|e| e.into_inner()).unlock_all();
                write_response(&mut stream, "200 OK", "application/json", br#"{"ok":true}"#)
            }
            ("POST", "/api/policy") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                match serde_json::from_slice::<RoomPolicyOverrides>(&body) {
                    Ok(policy) => {
                        store
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .set_policy(policy.clone());
                        let json = serde_json::to_vec(&policy).unwrap_or_default();
                        write_response(&mut stream, "200 OK", "application/json", &json)
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid policy payload"}"#,
                    ),
                }
            }
            ("POST", "/api/credentials") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                match serde_json::from_slice::<CredentialUpdateRequest>(&body) {
                    Ok(req) => {
                        let trimmed = req
                            .value
                            .as_deref()
                            .map(str::trim)
                            .filter(|v| !v.is_empty())
                            .map(str::to_string);
                        let hashed = match trimmed.as_deref() {
                            Some(v) => match hash_password(v) {
                                Ok(h) => Some(h),
                                Err(_) => {
                                    return write_response(
                                        &mut stream,
                                        "500 Internal Server Error",
                                        "application/json",
                                        br#"{"error":"failed to hash credential"}"#,
                                    );
                                }
                            },
                            None => None,
                        };
                        let mut guard = store.lock().unwrap_or_else(|e| e.into_inner());
                        match req.kind.as_str() {
                            "quit_password" => {
                                guard.policy.quit_password_hash = hashed;
                                guard.quit_password_plain = trimmed;
                            }
                            "entrance_code" => {
                                guard.policy.entrance_code_hash = hashed;
                                guard.entrance_code_plain = trimmed;
                            }
                            _ => {
                                return write_response(
                                    &mut stream,
                                    "400 Bad Request",
                                    "application/json",
                                    br#"{"error":"unknown credential kind"}"#,
                                );
                            }
                        }
                        let json = serde_json::to_vec(&guard.policy).unwrap_or_default();
                        write_response(&mut stream, "200 OK", "application/json", &json)
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid credentials payload"}"#,
                    ),
                }
            }
            ("POST", "/api/unlock") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                match serde_json::from_slice::<UnlockRequest>(&body) {
                    Ok(req) => {
                        let ok = store
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .unlock_student(&req.student_id);
                        let payload = if ok {
                            br#"{"ok":true}"#.as_slice()
                        } else {
                            br#"{"ok":false}"#.as_slice()
                        };
                        write_response(&mut stream, "200 OK", "application/json", payload)
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid unlock payload"}"#,
                    ),
                }
            }
            _ => write_response(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"Not Found",
            ),
        };
    }
}

fn serve_mjpeg_stream(stream: &mut TcpStream, channel: SharedWebcamChannel) -> std::io::Result<()> {
    let _ = stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT));
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-store, no-cache, must-revalidate\r\nConnection: close\r\n\r\n",
    )?;
    stream.flush()?;

    let mut last_seq = 0u64;
    loop {
        let frame = {
            let mut guard = channel.0.lock().unwrap_or_else(|e| e.into_inner());
            if guard.closed {
                break;
            }
            if guard.seq == last_seq || guard.jpeg.is_none() {
                let (g, _) = channel
                    .1
                    .wait_timeout(guard, Duration::from_secs(2))
                    .unwrap_or_else(|e| e.into_inner());
                guard = g;
            }
            if guard.closed {
                break;
            }
            if guard.seq != last_seq
                && let Some(bytes) = guard.jpeg.clone()
            {
                last_seq = guard.seq;
                Some(bytes)
            } else {
                None
            }
        };

        if let Some(bytes) = frame {
            let header = format!(
                "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            );
            if stream.write_all(header.as_bytes()).is_err()
                || stream.write_all(&bytes).is_err()
                || stream.write_all(b"\r\n").is_err()
                || stream.flush().is_err()
            {
                break;
            }
        }
    }
    Ok(())
}

fn decode_base64_jpeg(data_url: &str) -> Option<Vec<u8>> {
    let b64 = data_url.strip_prefix("data:image/jpeg;base64,")?;
    let mut out = Vec::with_capacity(b64.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for byte in b64.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(val);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex_str, 16)
        {
            out.push(val);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hb(id: &str, key: &str, violations: Vec<String>) -> StudentHeartbeat {
        StudentHeartbeat {
            student_id: id.to_string(),
            room_code: Some("ROOM-1".to_string()),
            exam_name: "Unit 4".to_string(),
            config_key: key.to_string(),
            status: StudentStatus::InExam,
            active_host: "canvas.school.edu".to_string(),
            violations,
        }
    }

    #[test]
    fn verifies_config_key_and_tracks_violations_webcam_and_unlock() {
        let mut store = RoomStore::new("ROOM-1".to_string(), Some("expected-key".to_string()));

        let r1 = store.ingest_heartbeat(sample_hb("alice", "expected-key", vec![]));
        assert!(r1.config_key_verified);
        assert_eq!(r1.command, ProctorCommand::None);

        assert!(store.ingest_webcam(WebcamFrameUpload {
            student_id: "alice".to_string(),
            data_url: "data:image/jpeg;base64,/9j/4AAQ".to_string(),
        }));
        assert_eq!(
            store.get_webcam("alice"),
            Some("data:image/jpeg;base64,/9j/4AAQ")
        );

        let r2 = store.ingest_heartbeat(sample_hb(
            "bob",
            "tampered-key",
            vec!["[displays] 2 displays".to_string()],
        ));
        assert!(!r2.config_key_verified);

        let snap = store.snapshot();
        assert_eq!(snap.total_students, 2);
        assert_eq!(snap.in_exam_count, 2);
        assert_eq!(snap.flagged_count, 1);
        assert!(snap.students.iter().find(|s| s.student_id == "alice").unwrap().has_webcam);

        assert!(store.unlock_student("alice"));
        let r3 = store.ingest_heartbeat(sample_hb("alice", "expected-key", vec![]));
        assert_eq!(r3.command, ProctorCommand::ForceUnlock);
        assert_eq!(store.get_webcam("alice"), None);
        assert!(
            !store
                .snapshot()
                .students
                .iter()
                .find(|s| s.student_id == "alice")
                .unwrap()
                .has_webcam
        );
        // Webcam uploads while Exited must be ignored.
        assert!(!store.ingest_webcam(WebcamFrameUpload {
            student_id: "alice".to_string(),
            data_url: "data:image/jpeg;base64,/9j/4AAQ".to_string(),
        }));
        assert_eq!(store.get_webcam("alice"), None);

        // Next heartbeat (e.g. if the student reopens the app) must NOT be force-unlocked again.
        let r4 = store.ingest_heartbeat(sample_hb("alice", "expected-key", vec![]));
        assert_eq!(r4.command, ProctorCommand::None);
    }

    #[test]
    fn exited_heartbeat_clears_and_blocks_webcam_uploads() {
        let mut store = RoomStore::new("ROOM-1".to_string(), None);
        store.ingest_heartbeat(sample_hb("carol", "k", vec![]));
        assert!(store.ingest_webcam(WebcamFrameUpload {
            student_id: "carol".to_string(),
            data_url: "data:image/webp;base64,UklGR".to_string(),
        }));
        assert!(store.get_webcam("carol").is_some());

        let mut exited_hb = sample_hb("carol", "k", vec![]);
        exited_hb.status = StudentStatus::Exited;
        store.ingest_heartbeat(exited_hb);

        assert_eq!(store.get_webcam("carol"), None);
        assert!(
            !store
                .snapshot()
                .students
                .iter()
                .find(|s| s.student_id == "carol")
                .unwrap()
                .has_webcam
        );
        assert!(!store.ingest_webcam(WebcamFrameUpload {
            student_id: "carol".to_string(),
            data_url: "data:image/webp;base64,UklGR".to_string(),
        }));
        assert_eq!(store.get_webcam("carol"), None);
    }

    const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn credentials_api_stores_returns_and_clears_plaintext_for_teacher() {
        let store = Arc::new(Mutex::new(RoomStore::new("ROOM-1".to_string(), None)));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_http = |req: String| -> String {
            let store_clone = Arc::clone(&store);
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(server_stream, &store_clone, TEST_TOKEN).unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
        };

        // Initially both null.
        let body = send_http(format!(
            "GET /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
        ));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v, serde_json::json!({"entrance_code": null, "quit_password": null}));

        // Set entrance_code and quit_password.
        let p1 = r#"{"kind":"entrance_code","value":"  PERIOD2  "}"#;
        send_http(format!(
            "POST /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{}",
            p1.len(),
            p1
        ));
        let p2 = r#"{"kind":"quit_password","value":"teacher-exit"}"#;
        send_http(format!(
            "POST /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{}",
            p2.len(),
            p2
        ));

        let body = send_http(format!(
            "GET /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
        ));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"entrance_code": "PERIOD2", "quit_password": "teacher-exit"})
        );

        // HeartbeatResponse must never contain plaintext credentials.
        let hb_resp = store
            .lock()
            .unwrap()
            .ingest_heartbeat(sample_hb("alice", "k", vec![]));
        let hb_json = serde_json::to_string(&hb_resp).unwrap();
        assert!(!hb_json.contains("PERIOD2"));
        assert!(!hb_json.contains("teacher-exit"));
        assert!(hb_resp.policy.entrance_code_hash.is_some());
        assert!(hb_resp.policy.quit_password_hash.is_some());

        // Clear entrance_code.
        let p3 = r#"{"kind":"entrance_code","value":null}"#;
        send_http(format!(
            "POST /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\nContent-Length: {}\r\n\r\n{}",
            p3.len(),
            p3
        ));
        let body = send_http(format!(
            "GET /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
        ));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"entrance_code": null, "quit_password": "teacher-exit"})
        );
    }

    #[test]
    fn ingest_webcam_raw_increments_seq_sets_has_webcam_and_exited_closes_channel() {
        let mut store = RoomStore::new("ROOM-1".to_string(), None);
        store.ingest_heartbeat(sample_hb("dave", "k", vec![]));

        let ch = store.webcam_channel("dave").expect("channel exists");
        {
            let guard = ch.0.lock().unwrap();
            assert_eq!(guard.seq, 0);
            assert!(guard.jpeg.is_none());
            assert!(!guard.closed);
        }

        let frame1 = vec![0xff, 0xd8, 0xff, 0xe0, 0x01];
        assert!(store.ingest_webcam_raw("dave".to_string(), frame1.clone()));
        assert!(
            store
                .snapshot()
                .students
                .iter()
                .find(|s| s.student_id == "dave")
                .unwrap()
                .has_webcam
        );
        {
            let guard = ch.0.lock().unwrap();
            assert_eq!(guard.seq, 1);
            assert_eq!(guard.jpeg.as_deref(), Some(frame1.as_slice()));
            assert!(!guard.closed);
        }

        let frame2 = vec![0xff, 0xd8, 0xff, 0xe0, 0x02];
        assert!(store.ingest_webcam_raw("dave".to_string(), frame2.clone()));
        {
            let guard = ch.0.lock().unwrap();
            assert_eq!(guard.seq, 2);
            assert_eq!(guard.jpeg.as_deref(), Some(frame2.as_slice()));
        }

        // Base64 JSON upload also decodes JPEG and advances seq.
        assert!(store.ingest_webcam(WebcamFrameUpload {
            student_id: "dave".to_string(),
            data_url: "data:image/jpeg;base64,/9j/4AAQ".to_string(),
        }));
        {
            let guard = ch.0.lock().unwrap();
            assert_eq!(guard.seq, 3);
            assert_eq!(
                guard.jpeg.as_deref(),
                Some(&[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10][..])
            );
        }

        // Exited heartbeat clears frame and closes the channel.
        let mut exited_hb = sample_hb("dave", "k", vec![]);
        exited_hb.status = StudentStatus::Exited;
        store.ingest_heartbeat(exited_hb);
        assert!(
            !store
                .snapshot()
                .students
                .iter()
                .find(|s| s.student_id == "dave")
                .unwrap()
                .has_webcam
        );
        {
            let guard = ch.0.lock().unwrap();
            assert!(guard.jpeg.is_none());
            assert!(guard.closed);
        }
        assert!(!store.ingest_webcam_raw("dave".to_string(), frame1));

        // Non-exited heartbeat resets closed = false.
        store.ingest_heartbeat(sample_hb("dave", "k", vec![]));
        assert!(!ch.0.lock().unwrap().closed);
    }

    #[test]
    fn webcam_raw_keep_alive_and_mjpeg_stream_deliver_frames() {
        let store = Arc::new(Mutex::new(RoomStore::new("ROOM-1".to_string(), None)));
        store
            .lock()
            .unwrap()
            .ingest_heartbeat(sample_hb("erin", "k", vec![]));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // 1. Send two raw frames over a single keep-alive TCP connection.
        let store_post = Arc::clone(&store);
        let mut post_client = TcpStream::connect(addr).unwrap();
        let (post_server, _) = listener.accept().unwrap();
        let post_handle = std::thread::spawn(move || {
            handle_connection(post_server, &store_post, TEST_TOKEN).unwrap();
        });

        let f1 = [0xffu8, 0xd8, 0x01, 0xff, 0xd9];
        let f2 = [0xffu8, 0xd8, 0x02, 0xff, 0xd9];
        let mut req1 = format!(
            "POST /api/webcam-raw/erin HTTP/1.1\r\nHost: localhost\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            f1.len()
        )
        .into_bytes();
        req1.extend_from_slice(&f1);
        post_client.write_all(&req1).unwrap();

        let mut post_reader = BufReader::new(post_client.try_clone().unwrap());
        let mut status1 = String::new();
        post_reader.read_line(&mut status1).unwrap();
        assert!(status1.starts_with("HTTP/1.1 200 OK"));
        let mut headers1 = String::new();
        loop {
            let mut l = String::new();
            post_reader.read_line(&mut l).unwrap();
            if l == "\r\n" {
                break;
            }
            headers1.push_str(&l);
        }
        assert!(headers1.to_ascii_lowercase().contains("keep-alive"));
        let mut ok1 = [0u8; 2];
        post_reader.read_exact(&mut ok1).unwrap();
        assert_eq!(&ok1, b"ok");

        // Second request on the SAME socket with Connection: close.
        let mut req2 = format!(
            "POST /api/webcam-raw/erin HTTP/1.1\r\nHost: localhost\r\nContent-Type: image/jpeg\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            f2.len()
        )
        .into_bytes();
        req2.extend_from_slice(&f2);
        post_client.write_all(&req2).unwrap();
        let mut resp2 = String::new();
        post_reader.read_to_string(&mut resp2).unwrap();
        assert!(resp2.starts_with("HTTP/1.1 200 OK"));
        assert!(resp2.ends_with("\r\n\r\nok"));
        post_handle.join().unwrap();

        // 2. Open GET /api/webcam-stream/erin, verify it streams the latest frame, then close on Exited.
        let store_stream = Arc::clone(&store);
        let mut viewer = TcpStream::connect(addr).unwrap();
        let (stream_server, _) = listener.accept().unwrap();
        let stream_handle = std::thread::spawn(move || {
            handle_connection(stream_server, &store_stream, TEST_TOKEN).unwrap();
        });

        viewer
            .write_all(
                format!(
                    "GET /api/webcam-stream/erin HTTP/1.1\r\nHost: localhost\r\nCookie: rb_proctor={TEST_TOKEN}\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();

        // Push one more frame, then mark student Exited so the MJPEG stream terminates cleanly.
        let store_notify = Arc::clone(&store);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            let mut exited = sample_hb("erin", "k", vec![]);
            exited.status = StudentStatus::Exited;
            store_notify.lock().unwrap().ingest_heartbeat(exited);
        });

        let mut stream_bytes = Vec::new();
        viewer.read_to_end(&mut stream_bytes).unwrap();
        stream_handle.join().unwrap();

        let stream_text = String::from_utf8_lossy(&stream_bytes);
        assert!(stream_text.contains("multipart/x-mixed-replace; boundary=frame"));
        assert!(stream_text.contains("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 5\r\n\r\n"));
        assert!(stream_bytes.windows(f2.len()).any(|w| w == f2));
    }

    #[test]
    fn screen_raw_ingest_stream_and_exited_closing() {
        let store = Arc::new(Mutex::new(RoomStore::new("ROOM-1".to_string(), None)));
        {
            let mut guard = store.lock().unwrap();
            guard.ingest_heartbeat(sample_hb("frank", "k", vec![]));
            assert!(
                !guard
                    .snapshot()
                    .students
                    .iter()
                    .find(|s| s.student_id == "frank")
                    .unwrap()
                    .has_screen
            );
            assert!(guard.ingest_screen_raw("frank", vec![0xff, 0xd8, 0xaa, 0xff, 0xd9]));
            assert!(
                guard
                    .snapshot()
                    .students
                    .iter()
                    .find(|s| s.student_id == "frank")
                    .unwrap()
                    .has_screen
            );
        }

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // 1. Send two screen frames over keep-alive POST /api/screen-raw/frank (including >512 KiB).
        let store_post = Arc::clone(&store);
        let mut post_client = TcpStream::connect(addr).unwrap();
        let (post_server, _) = listener.accept().unwrap();
        let post_handle = std::thread::spawn(move || {
            handle_connection(post_server, &store_post, TEST_TOKEN).unwrap();
        });

        let mut large_frame = vec![0x55u8; 600 * 1024];
        large_frame[0] = 0xff;
        large_frame[1] = 0xd8;
        let req1_hdr = format!(
            "POST /api/screen-raw/frank HTTP/1.1\r\nHost: localhost\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
            large_frame.len()
        );
        post_client.write_all(req1_hdr.as_bytes()).unwrap();
        post_client.write_all(&large_frame).unwrap();

        let mut post_reader = BufReader::new(post_client.try_clone().unwrap());
        let mut status1 = String::new();
        post_reader.read_line(&mut status1).unwrap();
        assert!(status1.starts_with("HTTP/1.1 200 OK"));
        let mut headers1 = String::new();
        loop {
            let mut l = String::new();
            post_reader.read_line(&mut l).unwrap();
            if l == "\r\n" {
                break;
            }
            headers1.push_str(&l);
        }
        assert!(headers1.to_ascii_lowercase().contains("keep-alive"));
        let mut ok1 = [0u8; 2];
        post_reader.read_exact(&mut ok1).unwrap();
        assert_eq!(&ok1, b"ok");

        let s2 = [0xffu8, 0xd8, 0x09, 0x08, 0xff, 0xd9];
        let mut req2 = format!(
            "POST /api/screen-raw/frank HTTP/1.1\r\nHost: localhost\r\nContent-Type: image/jpeg\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            s2.len()
        )
        .into_bytes();
        req2.extend_from_slice(&s2);
        post_client.write_all(&req2).unwrap();
        let mut resp2 = String::new();
        post_reader.read_to_string(&mut resp2).unwrap();
        assert!(resp2.starts_with("HTTP/1.1 200 OK"));
        assert!(resp2.ends_with("\r\n\r\nok"));
        post_handle.join().unwrap();

        // 2. Open GET /api/screen-stream/frank, verify it streams s2, then closes on Exited.
        let store_stream = Arc::clone(&store);
        let mut viewer = TcpStream::connect(addr).unwrap();
        let (stream_server, _) = listener.accept().unwrap();
        let stream_handle = std::thread::spawn(move || {
            handle_connection(stream_server, &store_stream, TEST_TOKEN).unwrap();
        });

        viewer
            .write_all(
                format!(
                    "GET /api/screen-stream/frank HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();

        let store_notify = Arc::clone(&store);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            let mut exited = sample_hb("frank", "k", vec![]);
            exited.status = StudentStatus::Exited;
            store_notify.lock().unwrap().ingest_heartbeat(exited);
        });

        let mut stream_bytes = Vec::new();
        viewer.read_to_end(&mut stream_bytes).unwrap();
        stream_handle.join().unwrap();

        let stream_text = String::from_utf8_lossy(&stream_bytes);
        assert!(stream_text.contains("multipart/x-mixed-replace; boundary=frame"));
        assert!(stream_text.contains("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 6\r\n\r\n"));
        assert!(stream_bytes.windows(s2.len()).any(|w| w == s2));

        // 3. After Exited, has_screen is false, channel is closed, uploads are rejected, and non-exited heartbeat reopens it.
        {
            let mut guard = store.lock().unwrap();
            assert!(
                !guard
                    .snapshot()
                    .students
                    .iter()
                    .find(|s| s.student_id == "frank")
                    .unwrap()
                    .has_screen
            );
            let ch = guard.screen_channel("frank").expect("screen channel exists");
            assert!(ch.0.lock().unwrap().closed);
            assert!(ch.0.lock().unwrap().jpeg.is_none());
            assert!(!guard.ingest_screen_raw("frank", s2.to_vec()));

            guard.ingest_heartbeat(sample_hb("frank", "k", vec![]));
            assert!(!ch.0.lock().unwrap().closed);
        }
    }

    #[test]
    fn admin_routes_require_proctor_auth_and_bootstrap_cookie_works() {
        let store = Arc::new(Mutex::new(RoomStore::new("ROOM-1".to_string(), None)));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let store_clone = Arc::clone(&store);
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(server_stream, &store_clone, TEST_TOKEN).unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        // Unauthenticated GET / returns 401 HTML.
        let r_root = send_raw("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        assert!(r_root.starts_with("HTTP/1.1 401 Unauthorized"));
        assert!(r_root.contains("text/html"));

        // Unauthenticated GET /api/credentials returns 401 JSON.
        let r_creds =
            send_raw("GET /api/credentials HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        assert!(r_creds.starts_with("HTTP/1.1 401 Unauthorized"));
        assert!(r_creds.ends_with(r#"{"error":"proctor authentication required"}"#));

        // Unauthenticated POST /api/unlock-all returns 401 JSON.
        let r_unlock =
            send_raw("POST /api/unlock-all HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        assert!(r_unlock.starts_with("HTTP/1.1 401 Unauthorized"));
        assert!(r_unlock.ends_with(r#"{"error":"proctor authentication required"}"#));

        // Bootstrap GET /?token=<TEST_TOKEN> sets HttpOnly SameSite=Strict cookie and redirects.
        let r_boot = send_raw(format!(
            "GET /?token={TEST_TOKEN} HTTP/1.1\r\nHost: localhost\r\n\r\n"
        ));
        assert!(r_boot.starts_with("HTTP/1.1 302 Found"));
        assert!(r_boot.contains("Location: /\r\n"));
        assert!(r_boot.contains(&format!(
            "Set-Cookie: rb_proctor={TEST_TOKEN}; Path=/; HttpOnly; SameSite=Strict"
        )));

        // Authenticated via Cookie succeeds on GET /api/credentials.
        let r_creds_ok = send_raw(format!(
            "GET /api/credentials HTTP/1.1\r\nHost: localhost\r\nCookie: foo=bar; rb_proctor={TEST_TOKEN}\r\n\r\n"
        ));
        assert!(r_creds_ok.starts_with("HTTP/1.1 200 OK"));

        // Authenticated via X-Proctor-Token succeeds on POST /api/unlock-all.
        let r_unlock_ok = send_raw(format!(
            "POST /api/unlock-all HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
        ));
        assert!(r_unlock_ok.starts_with("HTTP/1.1 200 OK"));
        assert!(r_unlock_ok.ends_with(r#"{"ok":true}"#));

        // Student telemetry POST /api/heartbeat remains open without auth.
        let hb_body = serde_json::to_string(&sample_hb("grace", "k", vec![])).unwrap();
        let r_hb = send_raw(format!(
            "POST /api/heartbeat HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            hb_body.len(),
            hb_body
        ));
        assert!(r_hb.starts_with("HTTP/1.1 200 OK"));
    }

    #[test]
    fn student_input_sanitization_and_store_caps() {
        let mut store = RoomStore::new("ROOM-1".to_string(), None);

        let long_id = "s".repeat(120);
        let many_violations: Vec<String> = (0..50).map(|i| format!("{i}-{}", "v".repeat(400))).collect();
        store.ingest_heartbeat(sample_hb(&long_id, "k", many_violations));

        let snap = store.snapshot();
        assert_eq!(snap.students.len(), 1);
        assert_eq!(snap.students[0].student_id.chars().count(), MAX_STUDENT_ID_CHARS);
        assert_eq!(snap.students[0].violations.len(), MAX_VIOLATIONS);
        assert!(
            snap.students[0]
                .violations
                .iter()
                .all(|v| v.chars().count() <= MAX_VIOLATION_CHARS)
        );

        for i in 0..(MAX_STUDENTS + 20) {
            store.ingest_heartbeat(sample_hb(&format!("student-{i}"), "k", vec![]));
        }
        assert_eq!(store.snapshot().total_students, MAX_STUDENTS);
    }

    #[test]
    fn bounded_headers_and_connection_guard_enforce_limits() {
        let store = Arc::new(Mutex::new(RoomStore::new("ROOM-1".to_string(), None)));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let store_clone = Arc::clone(&store);
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(server_stream, &store_clone, TEST_TOKEN).unwrap();
            });
            let _ = client.write_all(req.as_bytes());
            let mut resp = String::new();
            let _ = client.read_to_string(&mut resp);
            handle.join().unwrap();
            resp
        };

        // Oversized header line (> 4096 bytes) -> 431.
        let huge_hdr = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(MAX_HEADER_LINE_BYTES + 32)
        );
        let r_big = send_raw(huge_hdr);
        assert!(r_big.starts_with("HTTP/1.1 431 Request Header Fields Too Large"));

        // Too many headers (> 64) -> 431.
        let mut many_hdrs = "GET / HTTP/1.1\r\n".to_string();
        for i in 0..=MAX_HEADERS {
            many_hdrs.push_str(&format!("X-H-{i}: v\r\n"));
        }
        many_hdrs.push_str("\r\n");
        let r_many = send_raw(many_hdrs);
        assert!(r_many.starts_with("HTTP/1.1 431 Request Header Fields Too Large"));

        // Connection guard caps at MAX_CONCURRENT_CONNECTIONS and decrements on drop.
        let active = Arc::new(AtomicUsize::new(0));
        let mut guards = Vec::new();
        for _ in 0..MAX_CONCURRENT_CONNECTIONS {
            guards.push(try_acquire_connection(&active).expect("slot available"));
        }
        assert!(try_acquire_connection(&active).is_none());
        guards.pop();
        assert!(try_acquire_connection(&active).is_some());
        drop(guards);
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn dashboard_esc_escapes_html_quotes() {
        let html = dashboard::dashboard_html();
        assert!(html.contains(r#".replace(/"/g,'&quot;').replace(/'/g,'&#39;')"#));
    }
}
