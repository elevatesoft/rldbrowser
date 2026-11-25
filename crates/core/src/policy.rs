use url::Url;

use crate::config::{ExamConfig, is_localhost};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavigationDecision {
    Allow,
    Block(BlockReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockReason {
    DisallowedScheme(String),
    MissingHost,
    ExplicitlyBlocked(String),
    NotInAllowlist(String),
}

impl std::fmt::Display for BlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DisallowedScheme(s) => write!(f, "scheme '{s}' is not permitted"),
            Self::MissingHost => write!(f, "URL has no host"),
            Self::ExplicitlyBlocked(h) => write!(f, "host '{h}' is explicitly blocked"),
            Self::NotInAllowlist(h) => write!(f, "host '{h}' is not in the exam allowlist"),
        }
    }
}

#[derive(Debug, Clone)]
enum DomainPattern {
    Exact(String),
    WildcardSubdomain(String),
}

impl DomainPattern {
    fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.trim().trim_end_matches('.').to_ascii_lowercase();
        if normalized.is_empty() {
            return None;
        }
        if let Some(suffix) = normalized.strip_prefix("*.") {
            if suffix.is_empty() || suffix.contains('*') {
                return None;
            }
            Some(Self::WildcardSubdomain(suffix.to_string()))
        } else if normalized.contains('*') {
            None
        } else {
            Some(Self::Exact(normalized))
        }
    }

    fn matches(&self, host: &str) -> bool {
        match self {
            Self::Exact(target) => host == target,
            Self::WildcardSubdomain(suffix) => {
                host.len() > suffix.len() + 1
                    && host.ends_with(suffix)
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct UrlPolicy {
    allowed: Vec<DomainPattern>,
    blocked: Vec<DomainPattern>,
    allow_insecure_localhost: bool,
}

impl UrlPolicy {
    pub fn from_config(config: &ExamConfig, dev_mode: bool) -> Self {
        let mut allowed = Vec::new();
        if let Some(start_host) = config.start_url.host_str()
            && let Some(pat) = DomainPattern::parse(start_host)
        {
            allowed.push(pat);
        }
        for pinned in &config.pinned_tabs {
            if let Some(pinned_host) = pinned.url.host_str()
                && let Some(pat) = DomainPattern::parse(pinned_host)
            {
                allowed.push(pat);
            }
        }
        for domain in &config.allowed_domains {
            if let Some(pat) = DomainPattern::parse(domain) {
                allowed.push(pat);
            }
        }

        let blocked = config
            .blocked_domains
            .iter()
            .filter_map(|d| DomainPattern::parse(d))
            .collect();

        Self {
            allowed,
            blocked,
            allow_insecure_localhost: config.lockdown.allow_insecure_localhost || dev_mode,
        }
    }

    pub fn evaluate_str(&self, raw_url: &str) -> NavigationDecision {
        if raw_url == "about:blank" {
            return NavigationDecision::Allow;
        }
        match Url::parse(raw_url) {
            Ok(url) => self.evaluate(&url),
            Err(_) => NavigationDecision::Block(BlockReason::DisallowedScheme(
                "unparseable".to_string(),
            )),
        }
    }

    pub fn evaluate(&self, url: &Url) -> NavigationDecision {
        if url.as_str() == "about:blank" {
            return NavigationDecision::Allow;
        }

        match url.scheme() {
            "https" => {}
            "http" if self.allow_insecure_localhost && is_localhost(url) => {}
            other => {
                return NavigationDecision::Block(BlockReason::DisallowedScheme(other.to_string()));
            }
        }

        let Some(raw_host) = url.host_str() else {
            return NavigationDecision::Block(BlockReason::MissingHost);
        };
        let host = raw_host.trim_end_matches('.').to_ascii_lowercase();

        if self.blocked.iter().any(|pat| pat.matches(&host)) {
            return NavigationDecision::Block(BlockReason::ExplicitlyBlocked(host));
        }

        if self.allowed.iter().any(|pat| pat.matches(&host)) {
            NavigationDecision::Allow
        } else {
            NavigationDecision::Block(BlockReason::NotInAllowlist(host))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{BrowserPolicy, PinnedTab};
    use crate::config::LockdownPolicy;
    use crate::security::SecurityPolicy;

    fn sample_config() -> ExamConfig {
        ExamConfig {
            exam_name: "Test".to_string(),
            start_url: Url::parse("https://lms.school.edu/quiz/1").unwrap(),
            allowed_domains: vec!["*.instructure.com".to_string(), "cdn.school.edu".to_string()],
            blocked_domains: vec!["chat.instructure.com".to_string()],
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
    fn allows_start_url_host_and_wildcards() {
        let policy = UrlPolicy::from_config(&sample_config(), false);
        assert_eq!(
            policy.evaluate_str("https://lms.school.edu/quiz/2"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://my.instructure.com/login"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://a.b.instructure.com/asset.js"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://cdn.school.edu/style.css"),
            NavigationDecision::Allow
        );
    }

    #[test]
    fn blocks_lookalike_domains_and_unlisted_hosts() {
        let policy = UrlPolicy::from_config(&sample_config(), false);
        assert!(matches!(
            policy.evaluate_str("https://evil-instructure.com/"),
            NavigationDecision::Block(BlockReason::NotInAllowlist(_))
        ));
        assert!(matches!(
            policy.evaluate_str("https://instructure.com/"),
            NavigationDecision::Block(BlockReason::NotInAllowlist(_))
        ));
        assert!(matches!(
            policy.evaluate_str("https://google.com/search?q=answers"),
            NavigationDecision::Block(BlockReason::NotInAllowlist(_))
        ));
    }

    #[test]
    fn blocklist_overrides_allowlist() {
        let policy = UrlPolicy::from_config(&sample_config(), false);
        assert!(matches!(
            policy.evaluate_str("https://chat.instructure.com/room"),
            NavigationDecision::Block(BlockReason::ExplicitlyBlocked(_))
        ));
    }

    #[test]
    fn blocks_dangerous_schemes() {
        let policy = UrlPolicy::from_config(&sample_config(), false);
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,<h1>hi</h1>",
            "mailto:friend@example.com",
            "http://lms.school.edu/quiz/1",
        ] {
            assert!(
                matches!(
                    policy.evaluate_str(bad),
                    NavigationDecision::Block(BlockReason::DisallowedScheme(_))
                ),
                "expected {bad} to be blocked by scheme"
            );
        }
    }

    #[test]
    fn allows_about_blank_and_dev_localhost() {
        let mut cfg = sample_config();
        cfg.allowed_domains.push("localhost".to_string());
        let policy = UrlPolicy::from_config(&cfg, true);

        assert_eq!(policy.evaluate_str("about:blank"), NavigationDecision::Allow);
        assert_eq!(
            policy.evaluate_str("http://localhost:3000/quiz"),
            NavigationDecision::Allow
        );
    }

    #[test]
    fn allows_pinned_tab_hosts_automatically() {
        let mut cfg = sample_config();
        cfg.pinned_tabs.push(PinnedTab {
            title: "Formula Sheet".to_string(),
            url: Url::parse("https://formulas.math.org/sheet.pdf").unwrap(),
        });
        let policy = UrlPolicy::from_config(&cfg, false);

        assert_eq!(
            policy.evaluate_str("https://formulas.math.org/other"),
            NavigationDecision::Allow
        );
        assert!(matches!(
            policy.evaluate_str("https://other.math.org/"),
            NavigationDecision::Block(BlockReason::NotInAllowlist(_))
        ));
    }

    #[test]
    fn allows_bare_and_wildcard_domain_pair_for_www_redirects() {
        let mut cfg = sample_config();
        cfg.allowed_domains
            .extend(["desmos.com".to_string(), "*.desmos.com".to_string()]);
        let policy = UrlPolicy::from_config(&cfg, false);

        assert_eq!(
            policy.evaluate_str("https://desmos.com/"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://www.desmos.com/calculator"),
            NavigationDecision::Allow
        );
        assert_eq!(
            policy.evaluate_str("https://student.desmos.com/"),
            NavigationDecision::Allow
        );
        assert!(matches!(
            policy.evaluate_str("https://evil-desmos.com/"),
            NavigationDecision::Block(BlockReason::NotInAllowlist(_))
        ));
    }
}
