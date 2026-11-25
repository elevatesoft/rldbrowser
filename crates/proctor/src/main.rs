#[allow(dead_code)]
mod accounts;
mod dashboard;
#[allow(dead_code)]
mod ws;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

    /// Path to persist teacher accounts and exam rooms JSON
    #[arg(long, default_value = "rld-proctor-data.json")]
    data: PathBuf,

    /// Require a valid per-room join token on student telemetry endpoints (enable in cloud deployments)
    #[arg(long, default_value_t = false)]
    require_join_token: bool,

    /// Optional static student join token override (accepted when --require-join-token is enabled)
    #[arg(long)]
    join_token: Option<String>,
}

const FRAME_TTL: Duration = Duration::from_secs(3);
const NO_STORE_HEADERS: &str =
    "Cache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\n";

fn zeroize_vec(buf: &mut Vec<u8>) {
    buf.fill(0);
    buf.clear();
}

fn zeroize_opt_vec(opt: &mut Option<Vec<u8>>) {
    if let Some(mut buf) = opt.take() {
        zeroize_vec(&mut buf);
    }
}

fn zeroize_opt_string(opt: &mut Option<String>) {
    if let Some(s) = opt.take() {
        let mut bytes = s.into_bytes();
        zeroize_vec(&mut bytes);
    }
}

#[derive(Debug, Default)]
struct WebcamChannel {
    seq: u64,
    jpeg: Option<Vec<u8>>,
    captured_at: Option<Instant>,
    closed: bool,
}

impl WebcamChannel {
    fn replace_frame(&mut self, jpeg: Vec<u8>, now: Instant) {
        zeroize_opt_vec(&mut self.jpeg);
        self.seq = self.seq.wrapping_add(1).max(1);
        self.jpeg = Some(jpeg);
        self.captured_at = Some(now);
        self.closed = false;
    }

    fn clear_frame(&mut self) {
        zeroize_opt_vec(&mut self.jpeg);
        self.captured_at = None;
    }

    fn is_fresh(&self, now: Instant) -> bool {
        self.jpeg.is_some()
            && self
                .captured_at
                .is_some_and(|ts| now.saturating_duration_since(ts) <= FRAME_TTL)
    }

    fn purge_if_expired(&mut self, now: Instant) {
        if self.jpeg.is_some()
            && self
                .captured_at
                .is_none_or(|ts| now.saturating_duration_since(ts) > FRAME_TTL)
        {
            self.clear_frame();
        }
    }
}

impl Drop for WebcamChannel {
    fn drop(&mut self) {
        self.clear_frame();
    }
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
    webcam_data_url_at: Option<Instant>,
    webcam_channel: SharedWebcamChannel,
    screen_channel: SharedWebcamChannel,
}

impl Drop for StudentRecord {
    fn drop(&mut self) {
        zeroize_opt_string(&mut self.webcam_data_url);
        self.webcam_data_url_at = None;
    }
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

    pub fn purge_expired_frames(&mut self) {
        let now = Instant::now();
        for rec in self.students.values_mut() {
            if rec.webcam_data_url.is_some()
                && rec
                    .webcam_data_url_at
                    .is_none_or(|ts| now.saturating_duration_since(ts) > FRAME_TTL)
            {
                zeroize_opt_string(&mut rec.webcam_data_url);
                rec.webcam_data_url_at = None;
            }
            {
                let mut ch = rec
                    .webcam_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                ch.purge_if_expired(now);
            }
            {
                let mut ch = rec
                    .screen_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                ch.purge_if_expired(now);
            }
        }
    }

    pub fn ingest_heartbeat(&mut self, mut hb: StudentHeartbeat) -> HeartbeatResponse {
        self.purge_expired_frames();
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
                webcam_data_url_at: None,
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
            zeroize_opt_string(&mut entry.webcam_data_url);
            entry.webcam_data_url_at = None;
            {
                let mut ch = entry
                    .webcam_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                ch.clear_frame();
                ch.closed = true;
                entry.webcam_channel.1.notify_all();
            }
            {
                let mut ch = entry
                    .screen_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                ch.clear_frame();
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
            let mut bytes = upload.data_url.into_bytes();
            zeroize_vec(&mut bytes);
            return false;
        }
        if !self.students.contains_key(&upload.student_id) && self.students.len() >= MAX_STUDENTS {
            let mut bytes = upload.data_url.into_bytes();
            zeroize_vec(&mut bytes);
            return false;
        }
        if self
            .students
            .get(&upload.student_id)
            .is_some_and(|rec| rec.status == StudentStatus::Exited)
        {
            let mut bytes = upload.data_url.into_bytes();
            zeroize_vec(&mut bytes);
            return false;
        }
        let decoded_jpeg = decode_base64_jpeg(&upload.data_url);
        let now = Instant::now();
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
                last_seen: now,
                force_unlock: false,
                webcam_data_url: None,
                webcam_data_url_at: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });
        zeroize_opt_string(&mut entry.webcam_data_url);
        entry.webcam_data_url = Some(upload.data_url);
        entry.webcam_data_url_at = Some(now);
        if let Some(jpeg_bytes) = decoded_jpeg {
            let mut ch = entry
                .webcam_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ch.replace_frame(jpeg_bytes, now);
            entry.webcam_channel.1.notify_all();
        }
        entry.last_seen = now;
        true
    }

    pub fn ingest_webcam_raw(&mut self, student_id: String, mut jpeg_bytes: Vec<u8>) -> bool {
        let student_id = sanitize_student_id(&student_id);
        if jpeg_bytes.is_empty() {
            return false;
        }
        if !self.students.contains_key(&student_id) && self.students.len() >= MAX_STUDENTS {
            zeroize_vec(&mut jpeg_bytes);
            return false;
        }
        if self
            .students
            .get(&student_id)
            .is_some_and(|rec| rec.status == StudentStatus::Exited)
        {
            zeroize_vec(&mut jpeg_bytes);
            return false;
        }
        let now = Instant::now();
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
                last_seen: now,
                force_unlock: false,
                webcam_data_url: None,
                webcam_data_url_at: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });
        {
            let mut ch = entry
                .webcam_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ch.replace_frame(jpeg_bytes, now);
            entry.webcam_channel.1.notify_all();
        }
        entry.last_seen = now;
        true
    }

    fn webcam_channel(&self, student_id: &str) -> Option<SharedWebcamChannel> {
        self.students
            .get(student_id)
            .map(|r| Arc::clone(&r.webcam_channel))
    }

    pub fn ingest_screen_raw(&mut self, student_id: &str, mut jpeg: Vec<u8>) -> bool {
        let student_id = sanitize_student_id(student_id);
        if jpeg.is_empty() {
            return false;
        }
        if !self.students.contains_key(&student_id) && self.students.len() >= MAX_STUDENTS {
            zeroize_vec(&mut jpeg);
            return false;
        }
        if self
            .students
            .get(&student_id)
            .is_some_and(|rec| rec.status == StudentStatus::Exited)
        {
            zeroize_vec(&mut jpeg);
            return false;
        }
        let now = Instant::now();
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
                last_seen: now,
                force_unlock: false,
                webcam_data_url: None,
                webcam_data_url_at: None,
                webcam_channel: new_webcam_channel(),
                screen_channel: new_webcam_channel(),
            });
        {
            let mut ch = entry
                .screen_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            ch.replace_frame(jpeg, now);
            entry.screen_channel.1.notify_all();
        }
        entry.last_seen = now;
        true
    }

    fn screen_channel(&self, student_id: &str) -> Option<SharedWebcamChannel> {
        self.students
            .get(student_id)
            .map(|r| Arc::clone(&r.screen_channel))
    }

    pub fn get_webcam(&self, student_id: &str) -> Option<&str> {
        let now = Instant::now();
        self.students.get(student_id).and_then(|r| {
            let fresh = r
                .webcam_data_url_at
                .is_some_and(|ts| now.saturating_duration_since(ts) <= FRAME_TTL);
            if fresh {
                r.webcam_data_url.as_deref()
            } else {
                None
            }
        })
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

    pub fn snapshot(&mut self) -> RoomStateSnapshot {
        self.purge_expired_frames();
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
            let has_fresh_data_url = rec.webcam_data_url.is_some()
                && rec
                    .webcam_data_url_at
                    .is_some_and(|ts| now.saturating_duration_since(ts) <= FRAME_TTL);
            let has_fresh_webcam = has_fresh_data_url
                || rec
                    .webcam_channel
                    .0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_fresh(now);
            let has_fresh_screen = rec
                .screen_channel
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_fresh(now);
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
                has_webcam: has_fresh_webcam,
                has_screen: has_fresh_screen,
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

#[derive(Debug)]
pub struct RoomRegistry {
    default_room: String,
    expected_config_key: Option<String>,
    default_policy: Option<RoomPolicyOverrides>,
    rooms: BTreeMap<String, Arc<Mutex<RoomStore>>>,
}

impl RoomRegistry {
    pub fn new(
        default_room: String,
        expected_config_key: Option<String>,
        default_policy: Option<RoomPolicyOverrides>,
    ) -> Self {
        let normalized_default = normalize_room_code(Some(&default_room), "DEFAULT");
        let mut initial = RoomStore::new(normalized_default.clone(), expected_config_key.clone());
        if let Some(ref policy) = default_policy {
            initial.set_policy(policy.clone());
        }
        let mut rooms = BTreeMap::new();
        rooms.insert(normalized_default.clone(), Arc::new(Mutex::new(initial)));
        Self {
            default_room: normalized_default,
            expected_config_key,
            default_policy,
            rooms,
        }
    }

    pub fn default_room(&self) -> &str {
        &self.default_room
    }

    pub fn get_or_create(&mut self, room_code: &str) -> Arc<Mutex<RoomStore>> {
        let key = normalize_room_code(Some(room_code), &self.default_room);
        if let Some(existing) = self.rooms.get(&key) {
            return Arc::clone(existing);
        }
        let mut store = RoomStore::new(key.clone(), self.expected_config_key.clone());
        if let Some(ref policy) = self.default_policy {
            store.set_policy(policy.clone());
        }
        let arc = Arc::new(Mutex::new(store));
        self.rooms.insert(key, Arc::clone(&arc));
        arc
    }

    pub fn remove(&mut self, room_code: &str) {
        let key = normalize_room_code(Some(room_code), &self.default_room);
        if key != self.default_room {
            self.rooms.remove(&key);
        }
    }
}

#[derive(Debug, Clone)]
struct ServerConfig {
    proctor_token: String,
    require_join_token: bool,
    cli_join_token: Option<String>,
}

const AUTH_RATE_LIMIT_MAX_ATTEMPTS: usize = 10;
const AUTH_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
struct AuthRateLimiter {
    attempts: Mutex<BTreeMap<String, Vec<Instant>>>,
}

impl AuthRateLimiter {
    fn new() -> Self {
        Self::default()
    }

    fn check_and_record(&self, client_ip: &str) -> bool {
        let now = Instant::now();
        let mut map = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        let entries = map.entry(client_ip.to_string()).or_default();
        entries.retain(|ts| now.saturating_duration_since(*ts) < AUTH_RATE_LIMIT_WINDOW);
        if entries.len() >= AUTH_RATE_LIMIT_MAX_ATTEMPTS {
            return false;
        }
        entries.push(now);
        true
    }
}

#[derive(Debug, Deserialize)]
struct IncomingWebcamUpload {
    #[serde(flatten)]
    upload: WebcamFrameUpload,
    #[serde(default)]
    room_code: Option<String>,
    #[serde(default)]
    join_token: Option<String>,
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

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    name: String,
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct CreateRoomRequest {
    name: String,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SelectRoomRequest {
    code: String,
}

#[derive(Debug, Deserialize)]
struct WsClientMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    student_id: Option<String>,
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
    extract_cookie_value(cookie_header, "rb_proctor")
        .is_some_and(|val| constant_time_eq(val.as_bytes(), expected.as_bytes()))
}

fn extract_cookie_value<'a>(cookie_header: &'a str, target_name: &str) -> Option<&'a str> {
    for part in cookie_header.split(';') {
        if let Some((name, val)) = part.trim().split_once('=')
            && name.trim() == target_name
        {
            return Some(val.trim());
        }
    }
    None
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

fn normalize_room_code(raw: Option<&str>, default_room: &str) -> String {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_uppercase)
        .unwrap_or_else(|| default_room.trim().to_ascii_uppercase())
}

fn extract_client_ip(
    stream: &TcpStream,
    x_real_ip: Option<&str>,
    x_forwarded_for: Option<&str>,
) -> String {
    let peer_ip = stream.peer_addr().ok().map(|a| a.ip());
    if peer_ip.is_some_and(|ip| ip.is_loopback()) {
        if let Some(real_ip) = x_real_ip.map(str::trim).filter(|s| !s.is_empty()) {
            return real_ip.to_string();
        }
        if let Some(first_fwd) = x_forwarded_for
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return first_fwd.to_string();
        }
    }
    peer_ip
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn is_teacher_authorized_for_room(
    token_auth: bool,
    teacher: Option<&accounts::TeacherPublicView>,
    room_code: &str,
    default_room: &str,
    accounts: &Arc<Mutex<accounts::AccountStore>>,
) -> bool {
    if token_auth {
        return true;
    }
    let Some(teacher) = teacher else {
        return false;
    };
    let guard = accounts.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_room_owner(&teacher.id, room_code) {
        return true;
    }
    // If another teacher owns this room, deny access.
    if guard.get_room(room_code).is_some() {
        return false;
    }
    // Allow accessing the unclaimed CLI default room (e.g. before a teacher creates their first room).
    room_code.eq_ignore_ascii_case(default_room)
}

fn verify_student_join_token(
    config: &ServerConfig,
    accounts: &Arc<Mutex<accounts::AccountStore>>,
    room_code: &str,
    provided_token: Option<&str>,
) -> bool {
    if !config.require_join_token {
        return true;
    }
    let Some(tok) = provided_token.map(str::trim).filter(|t| !t.is_empty()) else {
        return false;
    };
    if let Some(cli_tok) = config.cli_join_token.as_deref().filter(|t| !t.is_empty())
        && constant_time_eq(tok.as_bytes(), cli_tok.as_bytes())
    {
        return true;
    }
    accounts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .verify_join_token(room_code, tok)
}

fn write_unauthorized_api(stream: &mut TcpStream) -> std::io::Result<()> {
    write_response(
        stream,
        "401 Unauthorized",
        "application/json",
        br#"{"error":"proctor authentication required"}"#,
    )
}

fn write_unauthorized_join_token(stream: &mut TcpStream) -> std::io::Result<()> {
    write_response(
        stream,
        "401 Unauthorized",
        "application/json",
        br#"{"error":"valid room join token required"}"#,
    )
}

fn write_forbidden_room(stream: &mut TcpStream) -> std::io::Result<()> {
    write_response(
        stream,
        "403 Forbidden",
        "application/json",
        br#"{"error":"forbidden: room belongs to another teacher"}"#,
    )
}

fn write_rate_limited(stream: &mut TcpStream) -> std::io::Result<()> {
    write_response_with_headers(
        stream,
        "429 Too Many Requests",
        "application/json",
        "Retry-After: 60\r\n",
        br#"{"error":"too many authentication attempts, please retry later"}"#,
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

    let default_policy = loaded_config.as_ref().map(RoomStore::policy_from_config);
    let registry = Arc::new(Mutex::new(RoomRegistry::new(
        cli.room,
        expected_key,
        default_policy,
    )));
    let accounts = Arc::new(Mutex::new(accounts::AccountStore::new(Some(cli.data))));
    let rate_limiter = Arc::new(AuthRateLimiter::new());
    let config = Arc::new(ServerConfig {
        proctor_token,
        require_join_token: cli.require_join_token,
        cli_join_token: cli.join_token.filter(|t| !t.trim().is_empty()),
    });
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
        let registry = Arc::clone(&registry);
        let accounts = Arc::clone(&accounts);
        let rate_limiter = Arc::clone(&rate_limiter);
        let config = Arc::clone(&config);
        std::thread::spawn(move || {
            let _guard = guard;
            let _ = handle_connection(stream, &registry, &accounts, &rate_limiter, &config);
        });
    }

    ExitCode::SUCCESS
}

fn handle_connection(
    mut stream: TcpStream,
    registry: &Arc<Mutex<RoomRegistry>>,
    accounts: &Arc<Mutex<accounts::AccountStore>>,
    rate_limiter: &Arc<AuthRateLimiter>,
    config: &ServerConfig,
) -> std::io::Result<()> {
    let proctor_token = config.proctor_token.as_str();
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
        let mut token_auth = false;
        let mut session_token: Option<String> = None;
        let mut forwarded_https = false;
        let mut upgrade_websocket = false;
        let mut ws_key: Option<String> = None;
        let mut header_room_code: Option<String> = None;
        let mut header_room_token: Option<String> = None;
        let mut header_real_ip: Option<String> = None;
        let mut header_forwarded_for: Option<String> = None;
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
                } else if name.eq_ignore_ascii_case("upgrade")
                    && val.eq_ignore_ascii_case("websocket")
                {
                    upgrade_websocket = true;
                } else if name.eq_ignore_ascii_case("sec-websocket-key") && !val.is_empty() {
                    ws_key = Some(val.to_string());
                } else if name.eq_ignore_ascii_case("x-forwarded-proto") {
                    if val
                        .split(',')
                        .next()
                        .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
                    {
                        forwarded_https = true;
                    }
                } else if name.eq_ignore_ascii_case("x-real-ip") && !val.is_empty() {
                    header_real_ip = Some(val.to_string());
                } else if name.eq_ignore_ascii_case("x-forwarded-for") && !val.is_empty() {
                    header_forwarded_for = Some(val.to_string());
                } else if name.eq_ignore_ascii_case("x-room-code") && !val.is_empty() {
                    header_room_code = Some(val.to_string());
                } else if name.eq_ignore_ascii_case("x-room-token") && !val.is_empty() {
                    header_room_token = Some(val.to_string());
                } else if name.eq_ignore_ascii_case("x-proctor-token")
                    && constant_time_eq(val.as_bytes(), proctor_token.as_bytes())
                {
                    token_auth = true;
                } else if name.eq_ignore_ascii_case("cookie") {
                    if cookie_has_proctor_token(val, proctor_token) {
                        token_auth = true;
                    }
                    if let Some(sess) = extract_cookie_value(val, "rb_session")
                        && !sess.is_empty()
                    {
                        session_token = Some(sess.to_string());
                    }
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

        let teacher = session_token.as_deref().and_then(|tok| {
            accounts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .authenticate(tok)
        });
        let authenticated = token_auth || teacher.is_some();
        let owner_id = teacher
            .as_ref()
            .map(|t| t.id.as_str())
            .unwrap_or("local-proctor");
        let secure_attr = if forwarded_https { "; Secure" } else { "" };
        let default_room = registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .default_room()
            .to_string();
        let query_room = query.and_then(|q| extract_query_param(q, "room"));
        let query_token = query.and_then(|q| extract_query_param(q, "token"));
        let admin_room = normalize_room_code(query_room.as_deref(), &default_room);
        let client_ip = extract_client_ip(
            &stream,
            header_real_ip.as_deref(),
            header_forwarded_for.as_deref(),
        );

        if method == "GET"
            && path == "/"
            && let Some(tok) = query_token.as_deref()
            && constant_time_eq(tok.as_bytes(), proctor_token.as_bytes())
        {
            let resp = format!(
                "HTTP/1.1 302 Found\r\nLocation: /\r\nSet-Cookie: rb_proctor={proctor_token}; Path=/; HttpOnly; SameSite=Strict{secure_attr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(resp.as_bytes())?;
            return stream.flush();
        }

        if method == "POST"
            && let Some(student_id) = path.strip_prefix("/api/webcam-raw/")
        {
            let target_room = normalize_room_code(
                header_room_code.as_deref().or(query_room.as_deref()),
                &default_room,
            );
            let provided_token = header_room_token.as_deref().or(query_token.as_deref());
            if !verify_student_join_token(config, accounts, &target_room, provided_token) {
                return write_unauthorized_join_token(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let room_store = registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_create(&target_room);
            room_store
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
            if !is_teacher_authorized_for_room(
                token_auth,
                teacher.as_ref(),
                &admin_room,
                &default_room,
                accounts,
            ) {
                return write_forbidden_room(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let room_store = registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_create(&admin_room);
            let channel = room_store
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
            let target_room = normalize_room_code(
                header_room_code.as_deref().or(query_room.as_deref()),
                &default_room,
            );
            let provided_token = header_room_token.as_deref().or(query_token.as_deref());
            if !verify_student_join_token(config, accounts, &target_room, provided_token) {
                return write_unauthorized_join_token(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let room_store = registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_create(&target_room);
            room_store
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
            if !is_teacher_authorized_for_room(
                token_auth,
                teacher.as_ref(),
                &admin_room,
                &default_room,
                accounts,
            ) {
                return write_forbidden_room(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let room_store = registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_create(&admin_room);
            let channel = room_store
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
            if !is_teacher_authorized_for_room(
                token_auth,
                teacher.as_ref(),
                &admin_room,
                &default_room,
                accounts,
            ) {
                return write_forbidden_room(&mut stream);
            }
            let decoded_id = percent_decode(student_id);
            let room_store = registry
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_create(&admin_room);
            let frame = {
                let mut guard = room_store.lock().unwrap_or_else(|e| e.into_inner());
                guard.purge_expired_frames();
                guard.get_webcam(&decoded_id).map(str::to_string)
            };
            let payload = serde_json::json!({ "data_url": frame }).to_string();
            return write_response_with_headers(
                &mut stream,
                "200 OK",
                "application/json",
                NO_STORE_HEADERS,
                payload.as_bytes(),
            );
        }

        if method == "DELETE"
            && let Some(room_code) = path.strip_prefix("/api/rooms/")
        {
            if !authenticated {
                return write_unauthorized_api(&mut stream);
            }
            let decoded_code = percent_decode(room_code);
            let ok = accounts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .delete_room(owner_id, &decoded_code);
            if ok {
                registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&decoded_code);
            }
            let payload = serde_json::json!({ "ok": ok }).to_string();
            return write_response(
                &mut stream,
                "200 OK",
                "application/json",
                payload.as_bytes(),
            );
        }

        return match (method, path) {
            ("GET", "/") => write_response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                dashboard::dashboard_html().as_bytes(),
            ),
            ("GET", "/ws/proctor") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
                }
                if !upgrade_websocket {
                    return write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"websocket upgrade required"}"#,
                    );
                }
                let Some(key) = ws_key else {
                    return write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"missing Sec-WebSocket-Key"}"#,
                    );
                };
                let room_store = registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_create(&admin_room);
                drop(reader);
                serve_proctor_ws(stream, &room_store, &key)
            }
            ("GET", "/api/auth/me") => {
                let has_accounts = accounts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .has_any_accounts();
                let payload = serde_json::json!({
                    "authenticated": authenticated,
                    "teacher": teacher,
                    "has_accounts": has_accounts,
                    "token_auth": token_auth,
                })
                .to_string();
                write_response(&mut stream, "200 OK", "application/json", payload.as_bytes())
            }
            ("POST", "/api/auth/register") => {
                if !rate_limiter.check_and_record(&client_ip) {
                    return write_rate_limited(&mut stream);
                }
                match serde_json::from_slice::<RegisterRequest>(&body) {
                    Ok(req) => {
                        let res = accounts
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .register(&req.name, &req.email, &req.password);
                        match res {
                            Ok((view, sess_tok)) => {
                                let payload = serde_json::json!({ "teacher": view }).to_string();
                                let cookie = format!(
                                    "Set-Cookie: rb_session={sess_tok}; Path=/; HttpOnly; SameSite=Strict{secure_attr}\r\n"
                                );
                                write_response_with_headers(
                                    &mut stream,
                                    "200 OK",
                                    "application/json",
                                    &cookie,
                                    payload.as_bytes(),
                                )
                            }
                            Err(err) => {
                                let payload = serde_json::json!({ "error": err }).to_string();
                                write_response(
                                    &mut stream,
                                    "400 Bad Request",
                                    "application/json",
                                    payload.as_bytes(),
                                )
                            }
                        }
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid registration payload"}"#,
                    ),
                }
            }
            ("POST", "/api/auth/login") => {
                if !rate_limiter.check_and_record(&client_ip) {
                    return write_rate_limited(&mut stream);
                }
                match serde_json::from_slice::<LoginRequest>(&body) {
                    Ok(req) => {
                        let res = accounts
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .login(&req.email, &req.password);
                        match res {
                            Ok((view, sess_tok)) => {
                                let payload = serde_json::json!({ "teacher": view }).to_string();
                                let cookie = format!(
                                    "Set-Cookie: rb_session={sess_tok}; Path=/; HttpOnly; SameSite=Strict{secure_attr}\r\n"
                                );
                                write_response_with_headers(
                                    &mut stream,
                                    "200 OK",
                                    "application/json",
                                    &cookie,
                                    payload.as_bytes(),
                                )
                            }
                            Err(err) => {
                                let payload = serde_json::json!({ "error": err }).to_string();
                                write_response(
                                    &mut stream,
                                    "401 Unauthorized",
                                    "application/json",
                                    payload.as_bytes(),
                                )
                            }
                        }
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid login payload"}"#,
                    ),
                }
            }
            ("POST", "/api/auth/logout") => {
                if let Some(tok) = session_token.as_deref() {
                    accounts
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .logout(tok);
                }
                let clear_cookies = format!(
                    "Set-Cookie: rb_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{secure_attr}\r\n\
                     Set-Cookie: rb_proctor=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{secure_attr}\r\n"
                );
                write_response_with_headers(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    &clear_cookies,
                    br#"{"ok":true}"#,
                )
            }
            ("GET", "/api/rooms") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                let rooms = accounts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .list_rooms_for_owner(owner_id);
                let payload = serde_json::json!({
                    "active_room": admin_room,
                    "rooms": rooms,
                })
                .to_string();
                write_response(&mut stream, "200 OK", "application/json", payload.as_bytes())
            }
            ("POST", "/api/rooms") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                match serde_json::from_slice::<CreateRoomRequest>(&body) {
                    Ok(req) => {
                        let custom_code = req
                            .code
                            .as_deref()
                            .map(str::trim)
                            .filter(|c| !c.is_empty());
                        let res = accounts
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .create_room(owner_id, &req.name, custom_code);
                        match res {
                            Ok(room) => {
                                registry
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .get_or_create(&room.code);
                                let json = serde_json::to_vec(&room).unwrap_or_default();
                                write_response(&mut stream, "200 OK", "application/json", &json)
                            }
                            Err(err) => {
                                let payload = serde_json::json!({ "error": err }).to_string();
                                write_response(
                                    &mut stream,
                                    "400 Bad Request",
                                    "application/json",
                                    payload.as_bytes(),
                                )
                            }
                        }
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid room payload"}"#,
                    ),
                }
            }
            ("POST", "/api/rooms/select") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                match serde_json::from_slice::<SelectRoomRequest>(&body) {
                    Ok(req) => {
                        let code = req.code.trim().to_ascii_uppercase();
                        if code.is_empty() {
                            return write_response(
                                &mut stream,
                                "400 Bad Request",
                                "application/json",
                                br#"{"error":"room code cannot be empty"}"#,
                            );
                        }
                        if !is_teacher_authorized_for_room(
                            token_auth,
                            teacher.as_ref(),
                            &code,
                            &default_room,
                            accounts,
                        ) {
                            return write_forbidden_room(&mut stream);
                        }
                        registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_create(&code);
                        let payload = serde_json::json!({
                            "ok": true,
                            "active_room": code,
                        })
                        .to_string();
                        write_response(
                            &mut stream,
                            "200 OK",
                            "application/json",
                            payload.as_bytes(),
                        )
                    }
                    Err(_) => write_response(
                        &mut stream,
                        "400 Bad Request",
                        "application/json",
                        br#"{"error":"invalid room selection payload"}"#,
                    ),
                }
            }
            ("GET", "/api/state") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
                }
                let room_store = registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_create(&admin_room);
                let snap = room_store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .snapshot();
                let json = serde_json::to_vec(&snap).unwrap_or_default();
                write_response(&mut stream, "200 OK", "application/json", &json)
            }
            ("GET", "/api/credentials") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
                }
                let room_store = registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_create(&admin_room);
                let creds = room_store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .credentials();
                let json = serde_json::to_vec(&creds).unwrap_or_default();
                write_response_with_headers(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    NO_STORE_HEADERS,
                    &json,
                )
            }
            ("POST", "/api/heartbeat") => {
                match serde_json::from_slice::<StudentHeartbeat>(&body) {
                    Ok(hb) => {
                        let target_room = normalize_room_code(
                            header_room_code
                                .as_deref()
                                .or(hb.room_code.as_deref()),
                            &default_room,
                        );
                        let provided_token =
                            header_room_token.as_deref().or(hb.join_token.as_deref());
                        if !verify_student_join_token(
                            config,
                            accounts,
                            &target_room,
                            provided_token,
                        ) {
                            return write_unauthorized_join_token(&mut stream);
                        }
                        let room_store = registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_create(&target_room);
                        let resp = room_store
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
                }
            }
            ("POST", "/api/webcam") => {
                match serde_json::from_slice::<IncomingWebcamUpload>(&body) {
                    Ok(incoming) => {
                        let target_room = normalize_room_code(
                            header_room_code
                                .as_deref()
                                .or(incoming.room_code.as_deref()),
                            &default_room,
                        );
                        let provided_token =
                            header_room_token.as_deref().or(incoming.join_token.as_deref());
                        if !verify_student_join_token(
                            config,
                            accounts,
                            &target_room,
                            provided_token,
                        ) {
                            return write_unauthorized_join_token(&mut stream);
                        }
                        let room_store = registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_create(&target_room);
                        let ok = room_store
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .ingest_webcam(incoming.upload);
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
                }
            }
            ("POST", "/api/unlock-all") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
                }
                let room_store = registry
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_or_create(&admin_room);
                room_store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .unlock_all();
                write_response(&mut stream, "200 OK", "application/json", br#"{"ok":true}"#)
            }
            ("POST", "/api/policy") => {
                if !authenticated {
                    return write_unauthorized_api(&mut stream);
                }
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
                }
                match serde_json::from_slice::<RoomPolicyOverrides>(&body) {
                    Ok(policy) => {
                        let room_store = registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_create(&admin_room);
                        room_store
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
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
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
                        let room_store = registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_create(&admin_room);
                        let mut guard = room_store.lock().unwrap_or_else(|e| e.into_inner());
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
                        write_response_with_headers(
                            &mut stream,
                            "200 OK",
                            "application/json",
                            NO_STORE_HEADERS,
                            &json,
                        )
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
                if !is_teacher_authorized_for_room(
                    token_auth,
                    teacher.as_ref(),
                    &admin_room,
                    &default_room,
                    accounts,
                ) {
                    return write_forbidden_room(&mut stream);
                }
                match serde_json::from_slice::<UnlockRequest>(&body) {
                    Ok(req) => {
                        let room_store = registry
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_create(&admin_room);
                        let ok = room_store
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

fn serve_proctor_ws(
    mut stream: TcpStream,
    store: &Arc<Mutex<RoomStore>>,
    ws_key: &str,
) -> std::io::Result<()> {
    ws::send_handshake(&mut stream, ws_key)?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT));

    let mut read_stream = stream.try_clone()?;
    let _ = read_stream.set_read_timeout(None);

    let focused_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let ws_closed = Arc::new(AtomicBool::new(false));
    let write_stream = Arc::new(Mutex::new(stream));

    let focused_reader = Arc::clone(&focused_id);
    let closed_reader = Arc::clone(&ws_closed);
    let pong_stream = Arc::clone(&write_stream);
    let reader_handle = std::thread::spawn(move || {
        while !closed_reader.load(Ordering::Acquire) {
            match ws::read_frame(&mut read_stream, 4096) {
                Ok(ws::WsIncoming::Text(text)) => {
                    if let Ok(msg) = serde_json::from_str::<WsClientMessage>(&text)
                        && msg.msg_type == "focus"
                    {
                        *focused_reader.lock().unwrap_or_else(|e| e.into_inner()) = msg.student_id;
                    }
                }
                Ok(ws::WsIncoming::Binary(_)) => {}
                Ok(ws::WsIncoming::Ping(payload)) => {
                    let mut ws_out = pong_stream.lock().unwrap_or_else(|e| e.into_inner());
                    if ws::write_pong_frame(&mut ws_out, &payload).is_err() {
                        break;
                    }
                }
                Ok(ws::WsIncoming::Close) => {
                    let mut ws_out = pong_stream.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = ws::write_close_frame(&mut ws_out);
                    break;
                }
                Err(_) => break,
            }
        }
        closed_reader.store(true, Ordering::Release);
    });

    let mut last_sent_cam_seq: BTreeMap<String, (u64, Instant)> = BTreeMap::new();
    let mut last_sent_screen_seq: BTreeMap<String, (u64, Instant)> = BTreeMap::new();
    let mut last_purge = Instant::now();

    while !ws_closed.load(Ordering::Acquire) {
        let now = Instant::now();
        let focused = focused_id.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let active: Vec<(String, SharedWebcamChannel, SharedWebcamChannel)> = {
            let mut guard = store.lock().unwrap_or_else(|e| e.into_inner());
            if now.saturating_duration_since(last_purge) >= Duration::from_millis(500) {
                guard.purge_expired_frames();
                last_purge = now;
            }
            guard
                .students
                .iter()
                .filter(|(_, rec)| rec.status != StudentStatus::Exited)
                .map(|(sid, rec)| {
                    (
                        sid.clone(),
                        Arc::clone(&rec.webcam_channel),
                        Arc::clone(&rec.screen_channel),
                    )
                })
                .collect()
        };

        let n = active.len();

        // Prune exited/removed students so rejoining students deliver their first frame immediately.
        last_sent_cam_seq.retain(|sid, _| active.iter().any(|(a, _, _)| a == sid));
        last_sent_screen_seq.retain(|sid, _| active.iter().any(|(a, _, _)| a == sid));

        let mut write_failed = false;
        for (sid, cam_ch, screen_ch) in &active {
            if ws_closed.load(Ordering::Acquire) {
                break;
            }
            let is_focused = focused.as_deref() == Some(sid.as_str()) || n <= 1;
            let min_cam_interval = if is_focused {
                Duration::ZERO
            } else {
                Duration::from_millis(180)
            };
            let min_screen_interval = if is_focused {
                Duration::ZERO
            } else {
                Duration::from_millis(450)
            };

            let cam_frame = {
                let mut ch = cam_ch.0.lock().unwrap_or_else(|e| e.into_inner());
                ch.purge_if_expired(now);
                let due = match last_sent_cam_seq.get(sid) {
                    None => true,
                    Some(&(last_seq, last_time)) => {
                        ch.seq > last_seq && now.duration_since(last_time) >= min_cam_interval
                    }
                };
                if !ch.closed && ch.seq > 0 && due && ch.is_fresh(now) {
                    ch.jpeg.clone().map(|jpeg| (ch.seq, jpeg))
                } else {
                    None
                }
            };
            if let Some((seq, mut jpeg)) = cam_frame {
                let mut packet = ws::encode_media_packet(ws::MEDIA_KIND_WEBCAM, sid, &jpeg);
                zeroize_vec(&mut jpeg);
                let res = {
                    let mut ws_out = write_stream.lock().unwrap_or_else(|e| e.into_inner());
                    ws::write_binary_frame(&mut ws_out, &packet)
                };
                zeroize_vec(&mut packet);
                if res.is_err() {
                    write_failed = true;
                    break;
                }
                last_sent_cam_seq.insert(sid.clone(), (seq, now));
            }

            let screen_frame = {
                let mut ch = screen_ch.0.lock().unwrap_or_else(|e| e.into_inner());
                ch.purge_if_expired(now);
                let due = match last_sent_screen_seq.get(sid) {
                    None => true,
                    Some(&(last_seq, last_time)) => {
                        ch.seq > last_seq && now.duration_since(last_time) >= min_screen_interval
                    }
                };
                if !ch.closed && ch.seq > 0 && due && ch.is_fresh(now) {
                    ch.jpeg.clone().map(|jpeg| (ch.seq, jpeg))
                } else {
                    None
                }
            };
            if let Some((seq, mut jpeg)) = screen_frame {
                let mut packet = ws::encode_media_packet(ws::MEDIA_KIND_SCREEN, sid, &jpeg);
                zeroize_vec(&mut jpeg);
                let res = {
                    let mut ws_out = write_stream.lock().unwrap_or_else(|e| e.into_inner());
                    ws::write_binary_frame(&mut ws_out, &packet)
                };
                zeroize_vec(&mut packet);
                if res.is_err() {
                    write_failed = true;
                    break;
                }
                last_sent_screen_seq.insert(sid.clone(), (seq, now));
            }
        }

        if write_failed {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }

    ws_closed.store(true, Ordering::Release);
    if let Ok(ws_out) = write_stream.lock() {
        let _ = ws_out.shutdown(Shutdown::Both);
    }
    let _ = reader_handle.join();
    Ok(())
}

fn serve_mjpeg_stream(stream: &mut TcpStream, channel: SharedWebcamChannel) -> std::io::Result<()> {
    let _ = stream.set_write_timeout(Some(CONNECTION_IO_TIMEOUT));
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nCache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\nConnection: close\r\n\r\n",
    )?;
    stream.flush()?;

    let mut last_seq = 0u64;
    loop {
        let frame = {
            let mut guard = channel.0.lock().unwrap_or_else(|e| e.into_inner());
            if guard.closed {
                break;
            }
            guard.purge_if_expired(Instant::now());
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
            let now = Instant::now();
            guard.purge_if_expired(now);
            if guard.seq != last_seq
                && guard.is_fresh(now)
                && let Some(bytes) = guard.jpeg.clone()
            {
                last_seq = guard.seq;
                Some(bytes)
            } else {
                None
            }
        };

        if let Some(mut bytes) = frame {
            let header = format!(
                "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            );
            let write_err = stream.write_all(header.as_bytes()).is_err()
                || stream.write_all(&bytes).is_err()
                || stream.write_all(b"\r\n").is_err()
                || stream.flush().is_err();
            zeroize_vec(&mut bytes);
            if write_err {
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
    write_response_with_headers(stream, status, content_type, "", body)
}

fn write_response_with_headers(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    extra_headers: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
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
            join_token: None,
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

    fn new_test_accounts() -> Arc<Mutex<accounts::AccountStore>> {
        Arc::new(Mutex::new(accounts::AccountStore::new(None)))
    }

    fn new_test_registry(default_room: &str) -> Arc<Mutex<RoomRegistry>> {
        Arc::new(Mutex::new(RoomRegistry::new(
            default_room.to_string(),
            None,
            None,
        )))
    }

    fn new_test_rate_limiter() -> Arc<AuthRateLimiter> {
        Arc::new(AuthRateLimiter::new())
    }

    fn default_test_config() -> ServerConfig {
        ServerConfig {
            proctor_token: TEST_TOKEN.to_string(),
            require_join_token: false,
            cli_join_token: None,
        }
    }

    #[test]
    fn credentials_api_stores_returns_and_clears_plaintext_for_teacher() {
        let registry = new_test_registry("ROOM-1");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_http = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
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
        let room_store = registry.lock().unwrap().get_or_create("ROOM-1");
        let hb_resp = room_store
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
        let registry = new_test_registry("ROOM-1");
        let store = registry.lock().unwrap().get_or_create("ROOM-1");
        store
            .lock()
            .unwrap()
            .ingest_heartbeat(sample_hb("erin", "k", vec![]));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        // 1. Send two raw frames over a single keep-alive TCP connection.
        let reg_post = Arc::clone(&registry);
        let accounts_post = new_test_accounts();
        let limiter_post = new_test_rate_limiter();
        let cfg_post = default_test_config();
        let mut post_client = TcpStream::connect(addr).unwrap();
        let (post_server, _) = listener.accept().unwrap();
        let post_handle = std::thread::spawn(move || {
            handle_connection(
                post_server,
                &reg_post,
                &accounts_post,
                &limiter_post,
                &cfg_post,
            )
            .unwrap();
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
        let reg_stream = Arc::clone(&registry);
        let accounts_stream = new_test_accounts();
        let limiter_stream = new_test_rate_limiter();
        let cfg_stream = default_test_config();
        let mut viewer = TcpStream::connect(addr).unwrap();
        let (stream_server, _) = listener.accept().unwrap();
        let stream_handle = std::thread::spawn(move || {
            handle_connection(
                stream_server,
                &reg_stream,
                &accounts_stream,
                &limiter_stream,
                &cfg_stream,
            )
            .unwrap();
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
        let registry = new_test_registry("ROOM-1");
        let store = registry.lock().unwrap().get_or_create("ROOM-1");
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
        let reg_post = Arc::clone(&registry);
        let accounts_post = new_test_accounts();
        let limiter_post = new_test_rate_limiter();
        let cfg_post = default_test_config();
        let mut post_client = TcpStream::connect(addr).unwrap();
        let (post_server, _) = listener.accept().unwrap();
        let post_handle = std::thread::spawn(move || {
            handle_connection(
                post_server,
                &reg_post,
                &accounts_post,
                &limiter_post,
                &cfg_post,
            )
            .unwrap();
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
        let reg_stream = Arc::clone(&registry);
        let accounts_stream = new_test_accounts();
        let limiter_stream = new_test_rate_limiter();
        let cfg_stream = default_test_config();
        let mut viewer = TcpStream::connect(addr).unwrap();
        let (stream_server, _) = listener.accept().unwrap();
        let stream_handle = std::thread::spawn(move || {
            handle_connection(
                stream_server,
                &reg_stream,
                &accounts_stream,
                &limiter_stream,
                &cfg_stream,
            )
            .unwrap();
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
        let registry = new_test_registry("ROOM-1");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        // Unauthenticated GET / serves the sign-in dashboard HTML shell (200 OK).
        let r_root = send_raw("GET / HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        assert!(r_root.starts_with("HTTP/1.1 200 OK"));
        assert!(r_root.contains("text/html"));
        assert!(r_root.contains("RLD Proctor"));

        // Unauthenticated GET /api/state returns 401 JSON.
        let r_state = send_raw("GET /api/state HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        assert!(r_state.starts_with("HTTP/1.1 401 Unauthorized"));
        assert!(r_state.ends_with(r#"{"error":"proctor authentication required"}"#));

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

        // Unauthenticated GET /api/rooms and GET /ws/proctor return 401 JSON.
        let r_rooms = send_raw("GET /api/rooms HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        assert!(r_rooms.starts_with("HTTP/1.1 401 Unauthorized"));
        let r_ws = send_raw(
            "GET /ws/proctor HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                .to_string(),
        );
        assert!(r_ws.starts_with("HTTP/1.1 401 Unauthorized"));

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
        let registry = new_test_registry("ROOM-1");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
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

    fn build_masked_ws_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x12u8, 0x34, 0x56, 0x78];
        let mut frame = Vec::new();
        frame.push(0x80 | opcode);
        assert!(payload.len() < 126);
        frame.push(0x80 | (payload.len() as u8));
        frame.extend_from_slice(&mask);
        for (i, &b) in payload.iter().enumerate() {
            frame.push(b ^ mask[i & 3]);
        }
        frame
    }

    #[test]
    fn teacher_auth_rooms_and_multiplexed_ws_flow() {
        let registry = new_test_registry("ROOM-1");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        // 1. Unauthenticated /api/auth/me reports authenticated=false, has_accounts=false.
        let r_me0 = send_raw("GET /api/auth/me HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string());
        let me0: serde_json::Value =
            serde_json::from_str(r_me0.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(me0["authenticated"], false);
        assert_eq!(me0["has_accounts"], false);

        // 2. Register teacher account -> 200 OK + Set-Cookie: rb_session=...
        let reg_body = r#"{"name":"Ms. Hopper","email":"grace@school.edu","password":"password123"}"#;
        let r_reg = send_raw(format!(
            "POST /api/auth/register HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            reg_body.len(),
            reg_body
        ));
        assert!(r_reg.starts_with("HTTP/1.1 200 OK"));
        let session_cookie = r_reg
            .lines()
            .find_map(|line| {
                line.strip_prefix("Set-Cookie: ")
                    .and_then(|v| v.split(';').next())
                    .filter(|c| c.starts_with("rb_session="))
                    .map(str::to_string)
            })
            .expect("rb_session cookie returned on register");

        // 3. Authenticated /api/auth/me returns teacher profile.
        let r_me1 = send_raw(format!(
            "GET /api/auth/me HTTP/1.1\r\nHost: localhost\r\nCookie: {session_cookie}\r\n\r\n"
        ));
        let me1: serde_json::Value =
            serde_json::from_str(r_me1.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(me1["authenticated"], true);
        assert_eq!(me1["teacher"]["email"], "grace@school.edu");
        assert_eq!(me1["has_accounts"], true);

        // 4. GET /api/rooms -> initially empty, active_room = ROOM-1.
        let r_rooms0 = send_raw(format!(
            "GET /api/rooms HTTP/1.1\r\nHost: localhost\r\nCookie: {session_cookie}\r\n\r\n"
        ));
        assert!(r_rooms0.starts_with("HTTP/1.1 200 OK"));
        let rooms0: serde_json::Value =
            serde_json::from_str(r_rooms0.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(rooms0["active_room"], "ROOM-1");
        assert_eq!(rooms0["rooms"].as_array().unwrap().len(), 0);

        // 5. POST /api/rooms -> creates room with a 32-char join token.
        let create_body = r#"{"name":"AP CS Period 2","code":"k7m-92p"}"#;
        let r_create = send_raw(format!(
            "POST /api/rooms HTTP/1.1\r\nHost: localhost\r\nCookie: {session_cookie}\r\nContent-Length: {}\r\n\r\n{}",
            create_body.len(),
            create_body
        ));
        assert!(r_create.starts_with("HTTP/1.1 200 OK"));
        let created: serde_json::Value =
            serde_json::from_str(r_create.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(created["code"], "K7M-92P");
        assert_eq!(created["name"], "AP CS Period 2");
        assert_eq!(created["join_token"].as_str().unwrap().len(), 32);

        // 6. GET /ws/proctor?room=K7M-92P with rb_session cookie -> 101 handshake + multiplexed webcam & screen frames.
        {
            let room_store = registry.lock().unwrap().get_or_create("K7M-92P");
            let mut guard = room_store.lock().unwrap();
            guard.ingest_heartbeat(sample_hb("stu1", "k", vec![]));
            guard.ingest_webcam_raw("stu1".to_string(), vec![0xff, 0xd8, 0x11, 0xff, 0xd9]);
            guard.ingest_screen_raw("stu1", vec![0xff, 0xd8, 0x22, 0x33, 0xff, 0xd9]);
        }

        let reg_ws = Arc::clone(&registry);
        let accounts_ws = Arc::clone(&accounts);
        let limiter_ws = Arc::clone(&limiter);
        let cfg_ws = config.clone();
        let mut ws_client = TcpStream::connect(addr).unwrap();
        ws_client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let (ws_server, _) = listener.accept().unwrap();
        let ws_handle = std::thread::spawn(move || {
            handle_connection(ws_server, &reg_ws, &accounts_ws, &limiter_ws, &cfg_ws).unwrap();
        });

        ws_client
            .write_all(
                format!(
                    "GET /ws/proctor?room=K7M-92P HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: {session_cookie}\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();

        let mut ws_reader = BufReader::new(ws_client.try_clone().unwrap());
        let mut handshake = String::new();
        loop {
            let mut line = String::new();
            ws_reader.read_line(&mut line).unwrap();
            handshake.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        assert!(handshake.starts_with("HTTP/1.1 101 Switching Protocols"));
        assert!(handshake.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));

        // Send a focus message from client to server.
        let focus_frame = build_masked_ws_frame(0x1, br#"{"type":"focus","student_id":"stu1"}"#);
        ws_client.write_all(&focus_frame).unwrap();

        // Read both webcam (kind=0) and screen (kind=1) binary frames.
        let mut got_cam = false;
        let mut got_screen = false;
        for _ in 0..4 {
            if let ws::WsIncoming::Binary(pkt) = ws::read_frame(&mut ws_reader, 64 * 1024).unwrap()
            {
                assert!(pkt.len() > 2);
                let kind = pkt[0];
                let id_len = pkt[1] as usize;
                let sid = std::str::from_utf8(&pkt[2..2 + id_len]).unwrap();
                let jpeg = &pkt[2 + id_len..];
                assert_eq!(sid, "stu1");
                if kind == ws::MEDIA_KIND_WEBCAM {
                    assert_eq!(jpeg, &[0xff, 0xd8, 0x11, 0xff, 0xd9]);
                    got_cam = true;
                } else if kind == ws::MEDIA_KIND_SCREEN {
                    assert_eq!(jpeg, &[0xff, 0xd8, 0x22, 0x33, 0xff, 0xd9]);
                    got_screen = true;
                }
            }
            if got_cam && got_screen {
                break;
            }
        }
        assert!(got_cam, "expected webcam binary frame over /ws/proctor");
        assert!(got_screen, "expected screen binary frame over /ws/proctor");

        // Close WebSocket cleanly.
        let close_frame = build_masked_ws_frame(0x8, &[]);
        ws_client.write_all(&close_frame).unwrap();
        drop(ws_reader);
        drop(ws_client);
        ws_handle.join().unwrap();

        // 7. DELETE /api/rooms/K7M-92P and POST /api/auth/logout.
        let r_del = send_raw(format!(
            "DELETE /api/rooms/K7M-92P HTTP/1.1\r\nHost: localhost\r\nCookie: {session_cookie}\r\n\r\n"
        ));
        assert!(r_del.starts_with("HTTP/1.1 200 OK"));
        assert!(r_del.ends_with(r#"{"ok":true}"#));

        let r_logout = send_raw(format!(
            "POST /api/auth/logout HTTP/1.1\r\nHost: localhost\r\nCookie: {session_cookie}\r\n\r\n"
        ));
        assert!(r_logout.starts_with("HTTP/1.1 200 OK"));
        assert!(r_logout.contains("Set-Cookie: rb_session=; Path=/; Max-Age=0"));

        let r_me2 = send_raw(format!(
            "GET /api/auth/me HTTP/1.1\r\nHost: localhost\r\nCookie: {session_cookie}\r\n\r\n"
        ));
        let me2: serde_json::Value =
            serde_json::from_str(r_me2.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(me2["authenticated"], false);
    }

    #[test]
    fn expired_frames_are_purged_and_zeroed_and_https_proxy_sets_secure_cookie() {
        // 1. Verify zeroize_vec clears bytes.
        let mut buf = vec![0xffu8, 0xd8, 0xaa, 0xbb];
        zeroize_vec(&mut buf);
        assert!(buf.is_empty());

        // 2. Verify frames older than FRAME_TTL are purged and has_webcam/has_screen become false.
        let mut store = RoomStore::new("ROOM-TTL".to_string(), None);
        store.ingest_heartbeat(sample_hb("zoe", "k", vec![]));
        assert!(store.ingest_webcam(WebcamFrameUpload {
            student_id: "zoe".to_string(),
            data_url: "data:image/jpeg;base64,/9j/4AAQ".to_string(),
        }));
        assert!(store.ingest_screen_raw("zoe", vec![0xff, 0xd8, 0x77, 0xff, 0xd9]));

        {
            let snap = store.snapshot();
            let s = snap.students.iter().find(|s| s.student_id == "zoe").unwrap();
            assert!(s.has_webcam);
            assert!(s.has_screen);
            assert!(store.get_webcam("zoe").is_some());
        }

        // Backdate frame timestamps past FRAME_TTL.
        let expired = Instant::now() - FRAME_TTL - Duration::from_secs(1);
        {
            let rec = store.students.get_mut("zoe").unwrap();
            rec.webcam_data_url_at = Some(expired);
            rec.webcam_channel.0.lock().unwrap().captured_at = Some(expired);
            rec.screen_channel.0.lock().unwrap().captured_at = Some(expired);
        }

        let snap_after = store.snapshot();
        let s_after = snap_after
            .students
            .iter()
            .find(|s| s.student_id == "zoe")
            .unwrap();
        assert!(!s_after.has_webcam);
        assert!(!s_after.has_screen);
        assert_eq!(store.get_webcam("zoe"), None);
        {
            let rec = store.students.get("zoe").unwrap();
            assert!(rec.webcam_data_url.is_none());
            assert!(rec.webcam_data_url_at.is_none());
            let cam = rec.webcam_channel.0.lock().unwrap();
            assert!(cam.jpeg.is_none());
            assert!(cam.captured_at.is_none());
            let scr = rec.screen_channel.0.lock().unwrap();
            assert!(scr.jpeg.is_none());
            assert!(scr.captured_at.is_none());
        }

        // 3. Verify X-Forwarded-Proto: https sets `; Secure` on auth cookies and no-store headers on media/credentials.
        let registry = new_test_registry("ROOM-TTL");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        // Bootstrap /?token=... behind HTTPS reverse proxy -> Secure cookie.
        let r_boot_https = send_raw(format!(
            "GET /?token={TEST_TOKEN} HTTP/1.1\r\nHost: proctor.example.edu\r\nX-Forwarded-Proto: https\r\n\r\n"
        ));
        assert!(r_boot_https.starts_with("HTTP/1.1 302 Found"));
        assert!(r_boot_https.contains(&format!(
            "Set-Cookie: rb_proctor={TEST_TOKEN}; Path=/; HttpOnly; SameSite=Strict; Secure\r\n"
        )));

        // Register behind HTTPS reverse proxy -> Secure cookie.
        let reg_body = r#"{"name":"Dr. Turing","email":"alan@school.edu","password":"password123"}"#;
        let r_reg_https = send_raw(format!(
            "POST /api/auth/register HTTP/1.1\r\nHost: proctor.example.edu\r\nX-Forwarded-Proto: https\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            reg_body.len(),
            reg_body
        ));
        assert!(r_reg_https.starts_with("HTTP/1.1 200 OK"));
        assert!(r_reg_https.contains("HttpOnly; SameSite=Strict; Secure\r\n"));

        // Login behind HTTPS reverse proxy -> Secure cookie.
        let login_body = r#"{"email":"alan@school.edu","password":"password123"}"#;
        let r_login_https = send_raw(format!(
            "POST /api/auth/login HTTP/1.1\r\nHost: proctor.example.edu\r\nX-Forwarded-Proto: https\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            login_body.len(),
            login_body
        ));
        assert!(r_login_https.starts_with("HTTP/1.1 200 OK"));
        assert!(r_login_https.contains("HttpOnly; SameSite=Strict; Secure\r\n"));

        // Plain HTTP login omits Secure so localhost works.
        let r_login_http = send_raw(format!(
            "POST /api/auth/login HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            login_body.len(),
            login_body
        ));
        assert!(r_login_http.starts_with("HTTP/1.1 200 OK"));
        assert!(!r_login_http.contains("; Secure"));

        // Cache-Control no-store + Pragma: no-cache on /api/credentials and /api/webcam/*.
        let r_creds = send_raw(format!(
            "GET /api/credentials HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
        ));
        assert!(r_creds.contains("Cache-Control: no-store, no-cache, must-revalidate, max-age=0\r\n"));
        assert!(r_creds.contains("Pragma: no-cache\r\n"));

        let r_cam = send_raw(format!(
            "GET /api/webcam/zoe HTTP/1.1\r\nHost: localhost\r\nX-Proctor-Token: {TEST_TOKEN}\r\n\r\n"
        ));
        assert!(r_cam.contains("Cache-Control: no-store, no-cache, must-revalidate, max-age=0\r\n"));
        assert!(r_cam.contains("Pragma: no-cache\r\n"));
    }

    fn extract_session_cookie(resp: &str) -> String {
        resp.lines()
            .find_map(|line| {
                line.strip_prefix("Set-Cookie: ")
                    .and_then(|v| v.split(';').next())
                    .filter(|c| c.starts_with("rb_session="))
                    .map(str::to_string)
            })
            .expect("rb_session cookie returned")
    }

    #[test]
    fn multi_tenant_room_isolation_forbids_cross_teacher_access() {
        let registry = new_test_registry("DEFAULT-ROOM");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        // Register Teacher A and Teacher B.
        let reg_a = r#"{"name":"Teacher A","email":"a@school.edu","password":"password123"}"#;
        let r_reg_a = send_raw(format!(
            "POST /api/auth/register HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            reg_a.len(),
            reg_a
        ));
        assert!(r_reg_a.starts_with("HTTP/1.1 200 OK"));
        let cookie_a = extract_session_cookie(&r_reg_a);

        let reg_b = r#"{"name":"Teacher B","email":"b@school.edu","password":"password123"}"#;
        let r_reg_b = send_raw(format!(
            "POST /api/auth/register HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            reg_b.len(),
            reg_b
        ));
        assert!(r_reg_b.starts_with("HTTP/1.1 200 OK"));
        let cookie_b = extract_session_cookie(&r_reg_b);

        // Teacher A creates ROOM-A, Teacher B creates ROOM-B.
        let create_a = r#"{"name":"Algebra","code":"ROOM-A"}"#;
        let r_ca = send_raw(format!(
            "POST /api/rooms HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_a}\r\nContent-Length: {}\r\n\r\n{}",
            create_a.len(),
            create_a
        ));
        assert!(r_ca.starts_with("HTTP/1.1 200 OK"));

        let create_b = r#"{"name":"Biology","code":"ROOM-B"}"#;
        let r_cb = send_raw(format!(
            "POST /api/rooms HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_b}\r\nContent-Length: {}\r\n\r\n{}",
            create_b.len(),
            create_b
        ));
        assert!(r_cb.starts_with("HTTP/1.1 200 OK"));

        // Student joins ROOM-A via heartbeat.
        let mut hb_a = sample_hb("stu-a", "k", vec![]);
        hb_a.room_code = Some("ROOM-A".to_string());
        let hb_a_body = serde_json::to_string(&hb_a).unwrap();
        let r_hb = send_raw(format!(
            "POST /api/heartbeat HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            hb_a_body.len(),
            hb_a_body
        ));
        assert!(r_hb.starts_with("HTTP/1.1 200 OK"));

        // Teacher A sees stu-a in ROOM-A; Teacher B sees 0 students in ROOM-B.
        let r_state_a = send_raw(format!(
            "GET /api/state?room=ROOM-A HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_a}\r\n\r\n"
        ));
        assert!(r_state_a.starts_with("HTTP/1.1 200 OK"));
        let state_a: serde_json::Value =
            serde_json::from_str(r_state_a.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(state_a["room_code"], "ROOM-A");
        assert_eq!(state_a["total_students"], 1);

        let r_state_b = send_raw(format!(
            "GET /api/state?room=ROOM-B HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_b}\r\n\r\n"
        ));
        assert!(r_state_b.starts_with("HTTP/1.1 200 OK"));
        let state_b: serde_json::Value =
            serde_json::from_str(r_state_b.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(state_b["room_code"], "ROOM-B");
        assert_eq!(state_b["total_students"], 0);

        // Teacher B gets 403 Forbidden on Teacher A's room for /api/state, /api/policy, /api/unlock-all, and /ws/proctor.
        let r_forbid_state = send_raw(format!(
            "GET /api/state?room=ROOM-A HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_b}\r\n\r\n"
        ));
        assert!(r_forbid_state.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(r_forbid_state.ends_with(r#"{"error":"forbidden: room belongs to another teacher"}"#));

        let pol_body = serde_json::to_string(&RoomPolicyOverrides::default()).unwrap();
        let r_forbid_policy = send_raw(format!(
            "POST /api/policy?room=ROOM-A HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_b}\r\nContent-Length: {}\r\n\r\n{}",
            pol_body.len(),
            pol_body
        ));
        assert!(r_forbid_policy.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(r_forbid_policy.ends_with(r#"{"error":"forbidden: room belongs to another teacher"}"#));

        let r_forbid_unlock = send_raw(format!(
            "POST /api/unlock-all?room=ROOM-A HTTP/1.1\r\nHost: localhost\r\nCookie: {cookie_b}\r\n\r\n"
        ));
        assert!(r_forbid_unlock.starts_with("HTTP/1.1 403 Forbidden"));
        assert!(r_forbid_unlock.ends_with(r#"{"error":"forbidden: room belongs to another teacher"}"#));

        let r_forbid_ws = send_raw(format!(
            "GET /ws/proctor?room=ROOM-A HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nCookie: {cookie_b}\r\n\r\n"
        ));
        assert!(r_forbid_ws.starts_with("HTTP/1.1 403 Forbidden"));
    }

    #[test]
    fn require_join_token_enforces_per_room_student_tokens() {
        let registry = new_test_registry("ROOM-SEC");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = ServerConfig {
            proctor_token: TEST_TOKEN.to_string(),
            require_join_token: true,
            cli_join_token: None,
        };

        let join_token = {
            let mut guard = accounts.lock().unwrap();
            let (teacher, _) = guard
                .register("Ms. Ada", "ada@school.edu", "password123")
                .unwrap();
            guard
                .create_room(&teacher.id, "Secure Room", Some("ROOM-SEC"))
                .unwrap()
                .join_token
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        let mut hb = sample_hb("stu-sec", "k", vec![]);
        hb.room_code = Some("ROOM-SEC".to_string());
        let hb_body = serde_json::to_string(&hb).unwrap();

        // Missing token -> 401 Unauthorized.
        let r_no_tok = send_raw(format!(
            "POST /api/heartbeat HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            hb_body.len(),
            hb_body
        ));
        assert!(r_no_tok.starts_with("HTTP/1.1 401 Unauthorized"));

        // Wrong token -> 401 Unauthorized.
        let r_bad_tok = send_raw(format!(
            "POST /api/heartbeat HTTP/1.1\r\nHost: localhost\r\nX-Room-Code: ROOM-SEC\r\nX-Room-Token: wrongtoken\r\nContent-Length: {}\r\n\r\n{}",
            hb_body.len(),
            hb_body
        ));
        assert!(r_bad_tok.starts_with("HTTP/1.1 401 Unauthorized"));

        // Valid token via X-Room-Token header -> 200 OK.
        let r_ok_hdr = send_raw(format!(
            "POST /api/heartbeat HTTP/1.1\r\nHost: localhost\r\nX-Room-Code: ROOM-SEC\r\nX-Room-Token: {join_token}\r\nContent-Length: {}\r\n\r\n{}",
            hb_body.len(),
            hb_body
        ));
        assert!(r_ok_hdr.starts_with("HTTP/1.1 200 OK"));

        // Valid token via JSON body join_token field -> 200 OK.
        let mut hb_val = serde_json::to_value(&hb).unwrap();
        hb_val["join_token"] = serde_json::Value::String(join_token.clone());
        let hb_json_tok = serde_json::to_string(&hb_val).unwrap();
        let r_ok_body = send_raw(format!(
            "POST /api/heartbeat HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            hb_json_tok.len(),
            hb_json_tok
        ));
        assert!(r_ok_body.starts_with("HTTP/1.1 200 OK"));

        // Webcam upload without token -> 401; with valid token -> 200 OK.
        let cam_body = r#"{"student_id":"stu-sec","data_url":"data:image/jpeg;base64,/9j/4AAQ"}"#;
        let r_cam_no_tok = send_raw(format!(
            "POST /api/webcam HTTP/1.1\r\nHost: localhost\r\nX-Room-Code: ROOM-SEC\r\nContent-Length: {}\r\n\r\n{}",
            cam_body.len(),
            cam_body
        ));
        assert!(r_cam_no_tok.starts_with("HTTP/1.1 401 Unauthorized"));

        let r_cam_ok = send_raw(format!(
            "POST /api/webcam HTTP/1.1\r\nHost: localhost\r\nX-Room-Code: ROOM-SEC\r\nX-Room-Token: {join_token}\r\nContent-Length: {}\r\n\r\n{}",
            cam_body.len(),
            cam_body
        ));
        assert!(r_cam_ok.starts_with("HTTP/1.1 200 OK"));

        // Raw webcam upload without token -> 401; with valid token -> 200 OK.
        let r_raw_no_tok = send_raw(
            "POST /api/webcam-raw/stu-sec HTTP/1.1\r\nHost: localhost\r\nX-Room-Code: ROOM-SEC\r\nConnection: close\r\nContent-Length: 4\r\n\r\n\x01\x02\x03\x04".to_string(),
        );
        assert!(r_raw_no_tok.starts_with("HTTP/1.1 401 Unauthorized"));

        let r_raw_ok = send_raw(format!(
            "POST /api/webcam-raw/stu-sec HTTP/1.1\r\nHost: localhost\r\nX-Room-Code: ROOM-SEC\r\nX-Room-Token: {join_token}\r\nConnection: close\r\nContent-Length: 4\r\n\r\n\x01\x02\x03\x04"
        ));
        assert!(r_raw_ok.starts_with("HTTP/1.1 200 OK"));
    }

    #[test]
    fn auth_rate_limiter_blocks_after_ten_attempts_per_ip() {
        let registry = new_test_registry("ROOM-RL");
        let accounts = new_test_accounts();
        let limiter = new_test_rate_limiter();
        let config = default_test_config();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let send_raw = |req: String| -> String {
            let reg_clone = Arc::clone(&registry);
            let accounts_clone = Arc::clone(&accounts);
            let limiter_clone = Arc::clone(&limiter);
            let cfg_clone = config.clone();
            let mut client = TcpStream::connect(addr).unwrap();
            let (server_stream, _) = listener.accept().unwrap();
            let handle = std::thread::spawn(move || {
                handle_connection(
                    server_stream,
                    &reg_clone,
                    &accounts_clone,
                    &limiter_clone,
                    &cfg_clone,
                )
                .unwrap();
            });
            client.write_all(req.as_bytes()).unwrap();
            let mut resp = String::new();
            client.read_to_string(&mut resp).unwrap();
            handle.join().unwrap();
            resp
        };

        let login_body = r#"{"email":"nobody@school.edu","password":"wrongpassword"}"#;
        for i in 0..AUTH_RATE_LIMIT_MAX_ATTEMPTS {
            let resp = send_raw(format!(
                "POST /api/auth/login HTTP/1.1\r\nHost: localhost\r\nX-Real-IP: 203.0.113.10\r\nContent-Length: {}\r\n\r\n{}",
                login_body.len(),
                login_body
            ));
            assert!(
                resp.starts_with("HTTP/1.1 401 Unauthorized"),
                "attempt {} expected 401, got: {}",
                i + 1,
                resp
            );
        }

        // 11th attempt from the same IP -> 429 Too Many Requests with Retry-After: 60.
        let r_limited = send_raw(format!(
            "POST /api/auth/login HTTP/1.1\r\nHost: localhost\r\nX-Real-IP: 203.0.113.10\r\nContent-Length: {}\r\n\r\n{}",
            login_body.len(),
            login_body
        ));
        assert!(r_limited.starts_with("HTTP/1.1 429 Too Many Requests"));
        assert!(r_limited.contains("Retry-After: 60\r\n"));

        // A different client IP behind the proxy is not rate-limited.
        let r_other_ip = send_raw(format!(
            "POST /api/auth/login HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 198.51.100.7, 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
            login_body.len(),
            login_body
        ));
        assert!(r_other_ip.starts_with("HTTP/1.1 401 Unauthorized"));
    }
}
