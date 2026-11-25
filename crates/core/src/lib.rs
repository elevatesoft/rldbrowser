pub mod browser;
pub mod config;
pub mod launch;
pub mod lockdown;
pub mod password;
pub mod policy;
pub mod proctor;
pub mod seb;
pub mod security;

pub use browser::{BrowserPolicy, PinnedTab, TabId, TabManager, TabState};
pub use config::{AccessibilityPolicy, ColorTint, ConfigError, ExamConfig, LockdownPolicy};
pub use launch::{LAUNCH_SCHEME, LaunchError, encode_config_launch_url, parse_launch_url};
pub use lockdown::{
    LockdownBackend, LockdownError, LockdownGuard, SecurityFinding, SecurityProbe, Severity,
};
pub use password::{PasswordError, hash_password, hash_password_with_salt, verify_password};
pub use policy::{BlockReason, NavigationDecision, UrlPolicy};
pub use proctor::{
    HeartbeatResponse, ProctorCommand, ProctorPolicy, RoomPolicyOverrides, RoomStateSnapshot,
    RoomStudentView, StudentHeartbeat, StudentStatus, WebcamFrameUpload,
};
pub use seb::{
    PlistValue, SebDocument, SebError, compute_seb_config_key, compute_seb_request_hash,
    parse_seb_bytes, parse_seb_file,
};
pub use security::{SecurityPolicy, SecurityReport, compute_config_key, evaluate_probes};
