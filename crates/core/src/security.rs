use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::ExamConfig;
use crate::lockdown::{SecurityFinding, SecurityProbe, Severity};

/// Policy controlling Phase 2 runtime hardening, assessment mode preference, and security probes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SecurityPolicy {
    pub prefer_assessment_mode: bool,
    pub deny_debugger_attach: bool,
    pub block_vm: bool,
    pub block_dylib_injection: bool,
    pub block_multiple_displays: bool,
    pub block_screen_capture_apps: bool,
    pub fail_closed: bool,
    pub monitor_interval_secs: u64,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            prefer_assessment_mode: true,
            deny_debugger_attach: true,
            block_vm: true,
            block_dylib_injection: true,
            block_multiple_displays: true,
            block_screen_capture_apps: true,
            fail_closed: true,
            monitor_interval_secs: 3,
        }
    }
}

/// Aggregated result of running security probes against a [`SecurityPolicy`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityReport {
    pub findings: Vec<SecurityFinding>,
}

impl SecurityReport {
    /// Returns `true` when no findings were recorded.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Returns `true` when `policy.fail_closed` is enabled and at least one finding is critical.
    pub fn has_blocking_findings(&self, policy: &SecurityPolicy) -> bool {
        policy.fail_closed
            && self
                .findings
                .iter()
                .any(|f| f.severity == Severity::Critical)
    }

    /// Iterates over all findings with [`Severity::Critical`].
    pub fn critical_findings(&self) -> impl Iterator<Item = &SecurityFinding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
    }

    /// Formats a human-readable summary of all findings in this report.
    pub fn format_summary(&self) -> String {
        if self.findings.is_empty() {
            return "Security check clean: 0 findings.".to_string();
        }

        let mut critical = 0usize;
        let mut warning = 0usize;
        let mut info = 0usize;
        for finding in &self.findings {
            match finding.severity {
                Severity::Critical => critical += 1,
                Severity::Warning => warning += 1,
                Severity::Info => info += 1,
            }
        }

        let mut lines = Vec::with_capacity(self.findings.len() + 1);
        lines.push(format!(
            "Security check found {} issue(s) ({critical} critical, {warning} warning, {info} info):",
            self.findings.len()
        ));
        for finding in &self.findings {
            let label = match finding.severity {
                Severity::Critical => "CRITICAL",
                Severity::Warning => "WARNING",
                Severity::Info => "INFO",
            };
            lines.push(format!("[{label}] {}: {}", finding.probe, finding.message));
        }
        lines.join("\n")
    }
}

/// Runs all provided probes and filters findings according to `policy`.
///
/// Known probe names (`"debugger"`, `"dylib-injection"`, `"vm"`, `"displays"`,
/// `"screen-capture"`) are gated by the corresponding [`SecurityPolicy`] flag.
/// Findings from unknown probe names are always retained.
pub fn evaluate_probes(probes: &[Box<dyn SecurityProbe>], policy: &SecurityPolicy) -> SecurityReport {
    let mut findings = Vec::new();
    for probe in probes {
        for finding in probe.check() {
            if is_probe_enabled(finding.probe, policy) {
                findings.push(finding);
            }
        }
    }
    SecurityReport { findings }
}

fn is_probe_enabled(probe_name: &str, policy: &SecurityPolicy) -> bool {
    match probe_name {
        "debugger" => policy.deny_debugger_attach,
        "dylib-injection" => policy.block_dylib_injection,
        "vm" => policy.block_vm,
        "displays" => policy.block_multiple_displays,
        "screen-capture" => policy.block_screen_capture_apps,
        _ => true,
    }
}

/// Computes a deterministic canonical SHA-256 hex digest ("Config Key") of an [`ExamConfig`].
///
/// Includes `start_url`, normalized sorted `allowed_domains` and `blocked_domains`,
/// `lockdown` policy, `security` policy, `browser` policy, `pinned_tabs` URLs,
/// and whether a quit password is configured.
/// Raw secrets such as `quit_password_hash` are never included in the hash input.
pub fn compute_config_key(config: &ExamConfig) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rustbrowser-config-key-v1\0");

    update_str(&mut hasher, config.start_url.as_str());

    let allowed = normalize_domains(&config.allowed_domains);
    update_str_list(&mut hasher, &allowed);

    let blocked = normalize_domains(&config.blocked_domains);
    update_str_list(&mut hasher, &blocked);

    let lockdown_flags = [
        u8::from(config.lockdown.kiosk_mode),
        u8::from(config.lockdown.block_new_windows),
        u8::from(config.lockdown.block_clipboard),
        u8::from(config.lockdown.block_context_menu),
        u8::from(config.lockdown.block_devtools),
        u8::from(config.lockdown.block_screen_capture),
        u8::from(config.lockdown.allow_back_forward),
        u8::from(config.lockdown.allow_reload),
        u8::from(config.lockdown.allow_insecure_localhost),
    ];
    hasher.update(lockdown_flags);

    let security_flags = [
        u8::from(config.security.prefer_assessment_mode),
        u8::from(config.security.deny_debugger_attach),
        u8::from(config.security.block_vm),
        u8::from(config.security.block_dylib_injection),
        u8::from(config.security.block_multiple_displays),
        u8::from(config.security.block_screen_capture_apps),
        u8::from(config.security.fail_closed),
    ];
    hasher.update(security_flags);
    hasher.update(config.security.monitor_interval_secs.to_be_bytes());

    let browser_flags = [
        u8::from(config.browser.ephemeral_session),
        u8::from(config.browser.block_downloads),
        u8::from(config.browser.allow_zoom),
        u8::from(config.browser.allow_find_in_page),
    ];
    hasher.update(browser_flags);
    match &config.browser.user_agent {
        Some(ua) => {
            hasher.update([1u8]);
            update_str(&mut hasher, ua);
        }
        None => hasher.update([0u8]),
    }

    let pinned_urls: Vec<String> = config
        .pinned_tabs
        .iter()
        .map(|tab| tab.url.as_str().to_string())
        .collect();
    update_str_list(&mut hasher, &pinned_urls);

    let accessibility_flags = [
        u8::from(config.accessibility.allow_read_aloud),
        u8::from(config.accessibility.allow_dictation),
        u8::from(config.accessibility.allow_spell_check),
        u8::from(config.accessibility.high_contrast),
        u8::from(config.accessibility.reduced_motion),
        u8::from(config.accessibility.line_focus),
        u8::from(config.accessibility.readable_spacing),
    ];
    hasher.update(accessibility_flags);
    update_str(&mut hasher, config.accessibility.color_tint.as_str());
    match &config.accessibility.read_aloud_voice {
        Some(voice) => {
            hasher.update([1u8]);
            update_str(&mut hasher, voice);
        }
        None => hasher.update([0u8]),
    }
    let rate_bps = (crate::config::AccessibilityPolicy::clamp_rate(
        config.accessibility.read_aloud_rate,
    ) * 100.0)
        .round() as u16;
    hasher.update(rate_bps.to_be_bytes());

    hasher.update([
        u8::from(config.entrance_code_hash.is_some()),
        u8::from(config.quit_password_hash.is_some()),
    ]);

    hex::encode(hasher.finalize())
}

fn normalize_domains(domains: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = domains
        .iter()
        .map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn update_str(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn update_str_list(hasher: &mut Sha256, values: &[String]) {
    hasher.update((values.len() as u64).to_be_bytes());
    for value in values {
        update_str(hasher, value);
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::browser::{BrowserPolicy, PinnedTab};
    use crate::config::LockdownPolicy;
    use crate::password::hash_password_with_salt;

    struct StubProbe {
        name: &'static str,
        findings: Vec<SecurityFinding>,
    }

    impl SecurityProbe for StubProbe {
        fn name(&self) -> &'static str {
            self.name
        }

        fn check(&self) -> Vec<SecurityFinding> {
            self.findings.clone()
        }
    }

    fn finding(probe: &'static str, severity: Severity, message: &str) -> SecurityFinding {
        SecurityFinding {
            probe,
            severity,
            message: message.to_string(),
        }
    }

    fn base_config() -> ExamConfig {
        ExamConfig {
            exam_name: "Midterm".to_string(),
            start_url: Url::parse("https://lms.example.edu/quiz/1").unwrap(),
            allowed_domains: vec!["*.example.edu".to_string(), "cdn.example.edu".to_string()],
            blocked_domains: vec!["chat.example.edu".to_string()],
            entrance_code_hash: None,
            quit_password_hash: None,
            lockdown: LockdownPolicy::default(),
            security: SecurityPolicy::default(),
            pinned_tabs: Vec::new(),
            browser: BrowserPolicy::default(),
            accessibility: crate::config::AccessibilityPolicy::default(),
            proctor: crate::proctor::ProctorPolicy::default(),
        }
    }

    #[test]
    fn security_policy_defaults_are_strict() {
        let policy = SecurityPolicy::default();
        assert!(policy.prefer_assessment_mode);
        assert!(policy.deny_debugger_attach);
        assert!(policy.block_vm);
        assert!(policy.block_dylib_injection);
        assert!(policy.block_multiple_displays);
        assert!(policy.block_screen_capture_apps);
        assert!(policy.fail_closed);
    }

    #[test]
    fn security_report_clean_and_blocking_behavior() {
        let policy = SecurityPolicy::default();
        let clean = SecurityReport::default();
        assert!(clean.is_clean());
        assert!(!clean.has_blocking_findings(&policy));
        assert_eq!(clean.critical_findings().count(), 0);
        assert_eq!(clean.format_summary(), "Security check clean: 0 findings.");

        let warn_only = SecurityReport {
            findings: vec![finding("vm", Severity::Warning, "VM suspected")],
        };
        assert!(!warn_only.is_clean());
        assert!(!warn_only.has_blocking_findings(&policy));
        assert_eq!(warn_only.critical_findings().count(), 0);

        let with_critical = SecurityReport {
            findings: vec![
                finding("vm", Severity::Warning, "VM suspected"),
                finding("debugger", Severity::Critical, "Debugger attached"),
            ],
        };
        assert!(!with_critical.is_clean());
        assert!(with_critical.has_blocking_findings(&policy));
        assert_eq!(with_critical.critical_findings().count(), 1);

        let fail_open = SecurityPolicy {
            fail_closed: false,
            ..SecurityPolicy::default()
        };
        assert!(!with_critical.has_blocking_findings(&fail_open));

        let summary = with_critical.format_summary();
        assert!(summary.contains("2 issue(s) (1 critical, 1 warning, 0 info)"));
        assert!(summary.contains("[CRITICAL] debugger: Debugger attached"));
        assert!(summary.contains("[WARNING] vm: VM suspected"));
    }

    #[test]
    fn evaluate_probes_filters_known_probes_and_keeps_unknown() {
        let probes: Vec<Box<dyn SecurityProbe>> = vec![
            Box::new(StubProbe {
                name: "debugger",
                findings: vec![finding("debugger", Severity::Critical, "traced")],
            }),
            Box::new(StubProbe {
                name: "dylib-injection",
                findings: vec![finding("dylib-injection", Severity::Critical, "DYLD_INSERT")],
            }),
            Box::new(StubProbe {
                name: "vm",
                findings: vec![finding("vm", Severity::Critical, "hypervisor")],
            }),
            Box::new(StubProbe {
                name: "displays",
                findings: vec![finding("displays", Severity::Warning, "2 displays")],
            }),
            Box::new(StubProbe {
                name: "screen-capture",
                findings: vec![finding("screen-capture", Severity::Critical, "OBS running")],
            }),
            Box::new(StubProbe {
                name: "custom-integrity",
                findings: vec![finding("custom-integrity", Severity::Critical, "tampered")],
            }),
        ];

        let all_enabled = evaluate_probes(&probes, &SecurityPolicy::default());
        assert_eq!(all_enabled.findings.len(), 6);

        let selective = SecurityPolicy {
            deny_debugger_attach: false,
            block_vm: false,
            block_screen_capture_apps: false,
            ..SecurityPolicy::default()
        };
        let filtered = evaluate_probes(&probes, &selective);
        let kept_names: Vec<&str> = filtered.findings.iter().map(|f| f.probe).collect();
        assert_eq!(
            kept_names,
            vec!["dylib-injection", "displays", "custom-integrity"]
        );
    }

    #[test]
    fn config_key_is_deterministic_normalized_and_excludes_secrets() {
        let cfg1 = base_config();
        let mut cfg2 = base_config();
        cfg2.exam_name = "Different Title".to_string();
        cfg2.allowed_domains = vec![
            "  CDN.EXAMPLE.EDU. ".to_string(),
            "*.example.edu".to_string(),
            "cdn.example.edu".to_string(),
        ];

        assert_eq!(cfg1.config_key(), cfg2.config_key());
        assert_eq!(cfg1.config_key().len(), 64);

        let mut cfg_with_pw1 = base_config();
        cfg_with_pw1.quit_password_hash = Some(hash_password_with_salt("pass1", b"0123456789abcdef"));
        let mut cfg_with_pw2 = base_config();
        cfg_with_pw2.quit_password_hash = Some(hash_password_with_salt("pass2", b"fedcba9876543210"));

        // Different raw password hashes produce the same key (only presence is hashed),
        // while differing from having no password set.
        assert_eq!(cfg_with_pw1.config_key(), cfg_with_pw2.config_key());
        assert_ne!(cfg1.config_key(), cfg_with_pw1.config_key());

        // Changing security or lockdown policy changes the config key.
        let mut cfg_sec_changed = base_config();
        cfg_sec_changed.security.block_vm = false;
        assert_ne!(cfg1.config_key(), cfg_sec_changed.config_key());

        let mut cfg_lock_changed = base_config();
        cfg_lock_changed.lockdown.kiosk_mode = false;
        assert_ne!(cfg1.config_key(), cfg_lock_changed.config_key());

        // Changing browser policy or pinned_tabs changes the config key.
        let mut cfg_browser_changed = base_config();
        cfg_browser_changed.browser.allow_find_in_page = true;
        assert_ne!(cfg1.config_key(), cfg_browser_changed.config_key());

        let mut cfg_ua_changed = base_config();
        cfg_ua_changed.browser.user_agent = Some("CustomUA/1.0".to_string());
        assert_ne!(cfg1.config_key(), cfg_ua_changed.config_key());

        let mut cfg_pinned_changed = base_config();
        cfg_pinned_changed.pinned_tabs.push(PinnedTab {
            title: "Notes".to_string(),
            url: Url::parse("https://notes.example.edu/").unwrap(),
        });
        assert_ne!(cfg1.config_key(), cfg_pinned_changed.config_key());
    }
}
