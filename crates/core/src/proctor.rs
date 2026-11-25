use serde::{Deserialize, Serialize};
use url::Url;

use crate::browser::PinnedTab;

/// Optional live classroom proctor telemetry settings in `exam.toml`.
/// When `server_url` is `None`, rustbrowser operates 100% locally with zero network telemetry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProctorPolicy {
    pub server_url: Option<Url>,
    pub room_code: Option<String>,
    pub heartbeat_interval_secs: u64,
    pub enable_webcam: bool,
}

impl Default for ProctorPolicy {
    fn default() -> Self {
        Self {
            server_url: None,
            room_code: None,
            heartbeat_interval_secs: 5,
            enable_webcam: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StudentStatus {
    PreflightBlocked,
    WaitingForCode,
    InExam,
    SecurityPaused,
    Exited,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProctorCommand {
    #[default]
    None,
    ForceUnlock,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RoomPolicyOverrides {
    /// Additional or live-managed allowed domain patterns (e.g. `*.instructure.com`, `desmos.com`).
    pub allowed_domains: Vec<String>,
    /// Explicitly blocked domain patterns (e.g. `chatgpt.com`).
    pub blocked_domains: Vec<String>,
    /// Reference tabs defined by the proctor.
    pub pinned_tabs: Vec<PinnedTab>,
    /// Optional salted hash for the exam entrance/access code.
    pub entrance_code_hash: Option<String>,
    /// Optional salted hash for the exam quit/exit password.
    pub quit_password_hash: Option<String>,
    pub require_webcam: bool,
    pub block_clipboard: bool,
    pub allow_read_aloud: bool,
    pub allow_find_in_page: bool,
    pub allow_back_forward: bool,
    pub allow_reload: bool,
    pub allow_zoom: bool,
    pub block_downloads: bool,
    pub block_multiple_displays: bool,
    pub block_screen_capture_apps: bool,
}

impl Default for RoomPolicyOverrides {
    fn default() -> Self {
        Self {
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            pinned_tabs: Vec::new(),
            entrance_code_hash: None,
            quit_password_hash: None,
            require_webcam: false,
            block_clipboard: true,
            allow_read_aloud: false,
            allow_find_in_page: false,
            allow_back_forward: false,
            allow_reload: true,
            allow_zoom: true,
            block_downloads: true,
            block_multiple_displays: true,
            block_screen_capture_apps: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StudentHeartbeat {
    pub student_id: String,
    pub room_code: Option<String>,
    pub exam_name: String,
    pub config_key: String,
    pub status: StudentStatus,
    pub active_host: String,
    pub violations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeartbeatResponse {
    pub command: ProctorCommand,
    pub config_key_verified: bool,
    #[serde(default)]
    pub policy: RoomPolicyOverrides,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebcamFrameUpload {
    pub student_id: String,
    pub data_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomStudentView {
    pub student_id: String,
    pub exam_name: String,
    pub config_key: String,
    pub config_key_verified: bool,
    pub status: StudentStatus,
    pub active_host: String,
    pub violations: Vec<String>,
    pub last_seen_secs_ago: u64,
    pub force_unlock_pending: bool,
    pub has_webcam: bool,
    #[serde(default)]
    pub has_screen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomStateSnapshot {
    pub room_code: String,
    pub expected_config_key: Option<String>,
    pub total_students: usize,
    pub in_exam_count: usize,
    pub flagged_count: usize,
    pub policy: RoomPolicyOverrides,
    pub students: Vec<RoomStudentView>,
}
