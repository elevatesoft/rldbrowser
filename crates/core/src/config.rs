use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::browser::{BrowserPolicy, PinnedTab};
use crate::proctor::ProctorPolicy;
use crate::security::{SecurityPolicy, compute_config_key};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse TOML config: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid start_url '{url}': {reason}")]
    InvalidStartUrl { url: String, reason: String },
    #[error("invalid pinned_tab url '{url}': {reason}")]
    InvalidPinnedTabUrl { url: String, reason: String },
    #[error("invalid quit_password_hash format (expected sha256$<salt_hex>$<digest_hex>)")]
    InvalidPasswordHash,
    #[error("invalid entrance_code_hash format (expected sha256$<salt_hex>$<digest_hex>)")]
    InvalidEntranceCodeHash,
    #[error("invalid accessibility.read_aloud_rate {0} (must be between 0.5 and 2.0)")]
    InvalidReadAloudRate(f64),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExamConfig {
    #[serde(default = "default_exam_name")]
    pub exam_name: String,
    pub start_url: Url,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    #[serde(default)]
    pub entrance_code_hash: Option<String>,
    #[serde(default)]
    pub quit_password_hash: Option<String>,
    #[serde(default)]
    pub lockdown: LockdownPolicy,
    #[serde(default)]
    pub security: SecurityPolicy,
    #[serde(default)]
    pub pinned_tabs: Vec<PinnedTab>,
    #[serde(default)]
    pub browser: BrowserPolicy,
    #[serde(default)]
    pub accessibility: AccessibilityPolicy,
    #[serde(default)]
    pub proctor: ProctorPolicy,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ColorTint {
    #[default]
    Default,
    Warm,
    Dark,
    Inverted,
}

impl ColorTint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Warm => "warm",
            Self::Dark => "dark",
            Self::Inverted => "inverted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AccessibilityPolicy {
    pub allow_read_aloud: bool,
    pub read_aloud_rate: f64,
    pub read_aloud_voice: Option<String>,
    pub allow_dictation: bool,
    pub allow_spell_check: bool,
    pub high_contrast: bool,
    pub reduced_motion: bool,
    pub color_tint: ColorTint,
    pub line_focus: bool,
    pub readable_spacing: bool,
}

impl Default for AccessibilityPolicy {
    fn default() -> Self {
        Self {
            allow_read_aloud: false,
            read_aloud_rate: 1.0,
            read_aloud_voice: None,
            allow_dictation: false,
            allow_spell_check: false,
            high_contrast: false,
            reduced_motion: false,
            color_tint: ColorTint::Default,
            line_focus: false,
            readable_spacing: false,
        }
    }
}

impl AccessibilityPolicy {
    pub fn clamp_rate(rate: f64) -> f64 {
        if !rate.is_finite() {
            return 1.0;
        }
        (rate.clamp(0.5, 2.0) * 100.0).round() / 100.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LockdownPolicy {
    pub kiosk_mode: bool,
    pub block_new_windows: bool,
    pub block_clipboard: bool,
    pub block_context_menu: bool,
    pub block_devtools: bool,
    pub block_screen_capture: bool,
    pub allow_back_forward: bool,
    pub allow_reload: bool,
    pub allow_insecure_localhost: bool,
}

impl Default for LockdownPolicy {
    fn default() -> Self {
        Self {
            kiosk_mode: true,
            block_new_windows: true,
            block_clipboard: true,
            block_context_menu: true,
            block_devtools: true,
            block_screen_capture: true,
            allow_back_forward: false,
            allow_reload: true,
            allow_insecure_localhost: false,
        }
    }
}

fn default_exam_name() -> String {
    "Locked Exam Session".to_string()
}

impl ExamConfig {
    pub fn from_toml_file(path: &Path) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        Self::from_toml_str(&content)
    }

    pub fn from_toml_str(raw: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(raw)?;
        config.validate()?;
        Ok(config)
    }

    pub fn for_url(start_url: Url) -> Result<Self, ConfigError> {
        let config = Self {
            exam_name: default_exam_name(),
            start_url,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            entrance_code_hash: None,
            quit_password_hash: None,
            lockdown: LockdownPolicy::default(),
            security: SecurityPolicy::default(),
            pinned_tabs: Vec::new(),
            browser: BrowserPolicy::default(),
            accessibility: AccessibilityPolicy::default(),
            proctor: ProctorPolicy::default(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_exam_url(
            &self.start_url,
            self.lockdown.allow_insecure_localhost,
            |url, reason| ConfigError::InvalidStartUrl { url, reason },
        )?;

        for pinned in &self.pinned_tabs {
            validate_exam_url(
                &pinned.url,
                self.lockdown.allow_insecure_localhost,
                |url, reason| ConfigError::InvalidPinnedTabUrl { url, reason },
            )?;
        }

        if let Some(hash) = &self.entrance_code_hash
            && !crate::password::is_valid_hash_format(hash)
        {
            return Err(ConfigError::InvalidEntranceCodeHash);
        }

        if let Some(hash) = &self.quit_password_hash
            && !crate::password::is_valid_hash_format(hash)
        {
            return Err(ConfigError::InvalidPasswordHash);
        }

        if !self.accessibility.read_aloud_rate.is_finite()
            || !(0.5..=2.0).contains(&self.accessibility.read_aloud_rate)
        {
            return Err(ConfigError::InvalidReadAloudRate(
                self.accessibility.read_aloud_rate,
            ));
        }

        Ok(())
    }

    pub fn requires_entrance_code(&self) -> bool {
        self.entrance_code_hash.is_some()
    }

    pub fn verify_entrance_code(&self, candidate: &str) -> bool {
        match &self.entrance_code_hash {
            Some(hash) => crate::password::verify_password(candidate, hash).unwrap_or(false),
            None => true,
        }
    }

    /// Computes a deterministic canonical SHA-256 hex digest ("Config Key") of this config.
    pub fn config_key(&self) -> String {
        compute_config_key(self)
    }
}

pub(crate) fn is_localhost(url: &Url) -> bool {
    matches!(
        url.host_str().map(|h| h.to_ascii_lowercase()).as_deref(),
        Some("localhost") | Some("127.0.0.1") | Some("[::1]") | Some("::1")
    )
}

fn validate_exam_url<F>(
    url: &Url,
    allow_insecure_localhost: bool,
    make_err: F,
) -> Result<(), ConfigError>
where
    F: FnOnce(String, String) -> ConfigError,
{
    match url.scheme() {
        "https" => {}
        "http" if allow_insecure_localhost && is_localhost(url) => {}
        other => {
            return Err(make_err(
                url.to_string(),
                format!("scheme '{other}' is not allowed (use https)"),
            ));
        }
    }

    if url.host_str().is_none() {
        return Err(make_err(
            url.to_string(),
            "URL must include a host".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::hash_password_with_salt;

    #[test]
    fn parses_valid_toml_config() {
        let hash = hash_password_with_salt("secret", b"0123456789abcdef");
        let toml = format!(
            r#"
exam_name = "Midterm"
start_url = "https://canvas.example.edu/courses/1/quizzes/2"
allowed_domains = ["*.example.edu"]
blocked_domains = ["chat.example.edu"]
quit_password_hash = "{hash}"

[lockdown]
kiosk_mode = true
block_clipboard = true
"#
        );

        let cfg = ExamConfig::from_toml_str(&toml).expect("valid config");
        assert_eq!(cfg.exam_name, "Midterm");
        assert_eq!(cfg.allowed_domains, vec!["*.example.edu"]);
        assert_eq!(cfg.blocked_domains, vec!["chat.example.edu"]);
        assert!(cfg.lockdown.kiosk_mode);
        assert!(cfg.lockdown.block_devtools);
    }

    #[test]
    fn rejects_insecure_http_non_localhost() {
        let toml = r#"
start_url = "http://example.edu/exam"
"#;
        let err = ExamConfig::from_toml_str(toml).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidStartUrl { .. }));
    }

    #[test]
    fn allows_http_localhost_when_configured() {
        let toml = r#"
start_url = "http://localhost:8080/exam"

[lockdown]
allow_insecure_localhost = true
"#;
        let cfg = ExamConfig::from_toml_str(toml).expect("localhost allowed");
        assert_eq!(cfg.start_url.host_str(), Some("localhost"));
    }

    #[test]
    fn rejects_malformed_password_hash() {
        let toml = r#"
start_url = "https://example.edu/exam"
quit_password_hash = "plaintext-not-allowed"
"#;
        let err = ExamConfig::from_toml_str(toml).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPasswordHash));
    }

    #[test]
    fn parses_security_policy_with_defaults() {
        let toml = r#"
start_url = "https://example.edu/exam"

[security]
block_vm = false
fail_closed = false
"#;
        let cfg = ExamConfig::from_toml_str(toml).expect("valid config with security");
        assert!(cfg.security.prefer_assessment_mode);
        assert!(cfg.security.deny_debugger_attach);
        assert!(!cfg.security.block_vm);
        assert!(cfg.security.block_dylib_injection);
        assert!(cfg.security.block_multiple_displays);
        assert!(cfg.security.block_screen_capture_apps);
        assert!(!cfg.security.fail_closed);
    }

    #[test]
    fn parses_and_validates_pinned_tabs_and_browser_policy() {
        let toml = r#"
start_url = "https://example.edu/exam"

[[pinned_tabs]]
title = "Reference"
url = "https://docs.example.edu/ref"

[browser]
allow_find_in_page = true
user_agent = "ExamBrowser/1.0"
"#;
        let cfg = ExamConfig::from_toml_str(toml).expect("valid config with browser & pinned tabs");
        assert_eq!(cfg.pinned_tabs.len(), 1);
        assert_eq!(cfg.pinned_tabs[0].title, "Reference");
        assert!(cfg.browser.ephemeral_session);
        assert!(cfg.browser.block_downloads);
        assert!(cfg.browser.allow_zoom);
        assert!(cfg.browser.allow_find_in_page);
        assert_eq!(cfg.browser.user_agent.as_deref(), Some("ExamBrowser/1.0"));

        let bad_pinned = r#"
start_url = "https://example.edu/exam"

[[pinned_tabs]]
title = "Insecure"
url = "http://docs.example.edu/ref"
"#;
        let err = ExamConfig::from_toml_str(bad_pinned).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidPinnedTabUrl { .. }));

        let localhost_pinned = r#"
start_url = "https://example.edu/exam"

[lockdown]
allow_insecure_localhost = true

[[pinned_tabs]]
title = "Local Docs"
url = "http://localhost:4000/docs"
"#;
        let cfg_local = ExamConfig::from_toml_str(localhost_pinned).expect("localhost pinned allowed");
        assert_eq!(cfg_local.pinned_tabs.len(), 1);
    }

    #[test]
    fn validates_and_verifies_entrance_code_hash() {
        let hash = hash_password_with_salt("EXAM-42", b"0123456789abcdef");
        let toml = format!(
            r#"
start_url = "https://example.edu/exam"
entrance_code_hash = "{hash}"
"#
        );
        let cfg = ExamConfig::from_toml_str(&toml).expect("valid entrance code config");
        assert!(cfg.requires_entrance_code());
        assert!(cfg.verify_entrance_code("EXAM-42"));
        assert!(!cfg.verify_entrance_code("WRONG"));

        let bad = r#"
start_url = "https://example.edu/exam"
entrance_code_hash = "raw-code-not-allowed"
"#;
        let err = ExamConfig::from_toml_str(bad).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidEntranceCodeHash));
    }

    #[test]
    fn parses_and_validates_accessibility_policy() {
        let toml = r#"
start_url = "https://example.edu/exam"

[accessibility]
allow_read_aloud = true
read_aloud_rate = 1.25
read_aloud_voice = "Samantha"
allow_dictation = true
allow_spell_check = true
high_contrast = true
reduced_motion = true
color_tint = "warm"
line_focus = true
readable_spacing = true
"#;
        let cfg = ExamConfig::from_toml_str(toml).expect("valid accessibility config");
        assert!(cfg.accessibility.allow_read_aloud);
        assert_eq!(cfg.accessibility.read_aloud_rate, 1.25);
        assert_eq!(cfg.accessibility.read_aloud_voice.as_deref(), Some("Samantha"));
        assert!(cfg.accessibility.allow_dictation);
        assert!(cfg.accessibility.allow_spell_check);
        assert!(cfg.accessibility.high_contrast);
        assert!(cfg.accessibility.reduced_motion);
        assert_eq!(cfg.accessibility.color_tint, ColorTint::Warm);
        assert!(cfg.accessibility.line_focus);
        assert!(cfg.accessibility.readable_spacing);

        let bad_rate = r#"
start_url = "https://example.edu/exam"

[accessibility]
read_aloud_rate = 3.5
"#;
        let err = ExamConfig::from_toml_str(bad_rate).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidReadAloudRate(_)));
    }
}
