use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::browser::BrowserPolicy;
use crate::config::{AccessibilityPolicy, ConfigError, ExamConfig, LockdownPolicy};
use crate::proctor::ProctorPolicy;
use crate::security::SecurityPolicy;

#[derive(Debug, Error)]
pub enum SebError {
    #[error("failed to read .seb file: {0}")]
    Io(#[from] std::io::Error),
    #[error("encrypted .seb files ({0}) are not supported yet; export an unencrypted .seb config")]
    EncryptedUnsupported(&'static str),
    #[error("failed to decompress gzipped .seb payload: {0}")]
    Decompress(String),
    #[error("invalid UTF-8 in .seb XML plist")]
    InvalidUtf8,
    #[error("failed to parse .seb XML plist: {0}")]
    XmlParse(String),
    #[error(".seb root must be a <dict>")]
    RootNotDict,
    #[error(".seb file is missing required 'startURL' key")]
    MissingStartUrl,
    #[error("invalid URL in .seb file: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("resulting exam configuration is invalid: {0}")]
    InvalidConfig(#[from] ConfigError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlistValue {
    Dict(BTreeMap<String, PlistValue>),
    Array(Vec<PlistValue>),
    String(String),
    Integer(i64),
    Real(f64),
    Bool(bool),
    Data(String),
}

#[derive(Debug, Clone)]
pub struct SebDocument {
    pub config: ExamConfig,
    /// Official Safe Exam Browser Config Key (SHA-256 hex of canonical JSON plist).
    pub seb_config_key: String,
}

/// Computes the official Safe Exam Browser request hash for `X-SafeExamBrowser-ConfigKeyHash`:
/// `SHA-256(url + seb_config_key)` encoded as lowercase hex.
pub fn compute_seb_request_hash(url: &str, seb_config_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    hasher.update(seb_config_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Parses a `.seb` file from disk into a validated [`ExamConfig`] and official SEB Config Key.
pub fn parse_seb_file(path: &Path) -> Result<SebDocument, SebError> {
    let bytes = fs::read(path)?;
    parse_seb_bytes(&bytes)
}

/// Parses raw `.seb` bytes (plain XML plist, `plnd` + gzipped XML plist, or raw gzipped XML plist).
pub fn parse_seb_bytes(bytes: &[u8]) -> Result<SebDocument, SebError> {
    let xml_bytes = unwrap_seb_payload(bytes)?;
    let xml = String::from_utf8(xml_bytes).map_err(|_| SebError::InvalidUtf8)?;
    let root = parse_xml_plist(&xml)?;
    let PlistValue::Dict(ref dict) = root else {
        return Err(SebError::RootNotDict);
    };

    let seb_config_key = compute_seb_config_key(dict);
    let config = map_seb_dict_to_config(dict)?;
    Ok(SebDocument {
        config,
        seb_config_key,
    })
}

fn unwrap_seb_payload(bytes: &[u8]) -> Result<Vec<u8>, SebError> {
    if bytes.len() >= 4 {
        match &bytes[..4] {
            b"pswd" => return Err(SebError::EncryptedUnsupported("pswd password-encrypted")),
            b"pwcc" => return Err(SebError::EncryptedUnsupported("pwcc password-encrypted")),
            b"pkhs" | b"phsk" => {
                return Err(SebError::EncryptedUnsupported("certificate-encrypted"));
            }
            b"plnd" => return decompress_if_gzip(&bytes[4..]),
            _ => {}
        }
    }
    decompress_if_gzip(bytes)
}

fn decompress_if_gzip(bytes: &[u8]) -> Result<Vec<u8>, SebError> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let mut child = Command::new("gzip")
            .args(["-dc"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SebError::Decompress(e.to_string()))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bytes)
                .map_err(|e| SebError::Decompress(e.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|e| SebError::Decompress(e.to_string()))?;
        if !output.status.success() {
            return Err(SebError::Decompress(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(output.stdout)
    } else {
        Ok(bytes.to_vec())
    }
}

/// Computes the official SEB Config Key: SHA-256 hex of the canonical JSON representation
/// of the plist dictionary (keys sorted, `originatorVersion` removed, empty dicts omitted).
pub fn compute_seb_config_key(dict: &BTreeMap<String, PlistValue>) -> String {
    let mut canonical = String::new();
    write_canonical_dict(dict, &mut canonical, true);
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn write_canonical_dict(
    dict: &BTreeMap<String, PlistValue>,
    out: &mut String,
    is_root: bool,
) {
    out.push('{');
    let mut first = true;
    for (k, v) in dict {
        if is_root && k == "originatorVersion" {
            continue;
        }
        if let PlistValue::Dict(inner) = v
            && inner.is_empty()
        {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string()));
        out.push(':');
        write_canonical_value(v, out);
    }
    out.push('}');
}

fn write_canonical_value(val: &PlistValue, out: &mut String) {
    match val {
        PlistValue::Dict(d) => write_canonical_dict(d, out, false),
        PlistValue::Array(arr) => {
            out.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical_value(item, out);
            }
            out.push(']');
        }
        PlistValue::String(s) | PlistValue::Data(s) => {
            out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
        }
        PlistValue::Integer(n) => out.push_str(&n.to_string()),
        PlistValue::Real(r) => out.push_str(&r.to_string()),
        PlistValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
    }
}

fn map_seb_dict_to_config(dict: &BTreeMap<String, PlistValue>) -> Result<ExamConfig, SebError> {
    let start_url_str = get_str(dict, "startURL").ok_or(SebError::MissingStartUrl)?;
    let start_url = Url::parse(start_url_str)?;

    let exam_name = get_str(dict, "examSessionName")
        .or_else(|| get_str(dict, "originatorName"))
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "SEB Exam ({})",
                start_url.host_str().unwrap_or("locked")
            )
        });

    let quit_password_hash = get_str(dict, "hashedQuitPassword")
        .map(str::trim)
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|h| format!("seb_sha256${}", h.to_ascii_lowercase()));

    let mut allowed_domains = Vec::new();
    let mut blocked_domains = Vec::new();
    if let Some(PlistValue::Array(rules)) = dict.get("URLFilterRules") {
        for rule in rules {
            if let PlistValue::Dict(rule_dict) = rule {
                let active = get_bool(rule_dict, "active").unwrap_or(true);
                if !active {
                    continue;
                }
                if let Some(expr) = get_str(rule_dict, "expression")
                    && let Some(host) = extract_domain_pattern(expr)
                {
                    // SEB action: 1 = allow, 0 = block
                    match get_int(rule_dict, "action").unwrap_or(1) {
                        0 => blocked_domains.push(host),
                        _ => allowed_domains.push(host),
                    }
                }
            }
        }
    }

    let lockdown = LockdownPolicy {
        kiosk_mode: true,
        block_new_windows: true,
        block_clipboard: true,
        block_context_menu: true,
        block_devtools: true,
        block_screen_capture: !get_bool(dict, "allowScreenCapture").unwrap_or(false),
        allow_back_forward: get_bool(dict, "allowBrowsingBackForward").unwrap_or(false),
        allow_reload: get_bool(dict, "browserWindowAllowReload").unwrap_or(true),
        allow_insecure_localhost: false,
    };

    let security = SecurityPolicy {
        block_vm: !get_bool(dict, "allowVirtualMachine").unwrap_or(false),
        block_screen_capture_apps: !get_bool(dict, "allowScreenSharing").unwrap_or(false),
        ..SecurityPolicy::default()
    };

    let user_agent = get_str(dict, "browserUserAgentMac")
        .or_else(|| get_str(dict, "browserUserAgent"))
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    let browser = BrowserPolicy {
        ephemeral_session: true,
        block_downloads: !get_bool(dict, "allowDownloads").unwrap_or(false),
        allow_zoom: true,
        allow_find_in_page: get_bool(dict, "allowFind").unwrap_or(false),
        user_agent,
    };

    let accessibility = AccessibilityPolicy {
        allow_dictation: get_bool(dict, "allowDictation").unwrap_or(false),
        allow_spell_check: get_bool(dict, "allowSpellCheck").unwrap_or(false),
        ..AccessibilityPolicy::default()
    };

    let config = ExamConfig {
        exam_name,
        start_url,
        allowed_domains,
        blocked_domains,
        entrance_code_hash: None,
        quit_password_hash,
        lockdown,
        security,
        pinned_tabs: Vec::new(),
        browser,
        accessibility,
        proctor: ProctorPolicy::default(),
    };
    config.validate()?;
    Ok(config)
}

fn extract_domain_pattern(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(u) = Url::parse(trimmed) {
        return u.host_str().map(str::to_string);
    }
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let host_part = without_scheme.split('/').next()?.split(':').next()?.trim();
    if host_part.is_empty() {
        None
    } else {
        Some(host_part.to_string())
    }
}

fn get_str<'a>(dict: &'a BTreeMap<String, PlistValue>, key: &str) -> Option<&'a str> {
    match dict.get(key) {
        Some(PlistValue::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn get_bool(dict: &BTreeMap<String, PlistValue>, key: &str) -> Option<bool> {
    match dict.get(key) {
        Some(PlistValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn get_int(dict: &BTreeMap<String, PlistValue>, key: &str) -> Option<i64> {
    match dict.get(key) {
        Some(PlistValue::Integer(n)) => Some(*n),
        _ => None,
    }
}

/// Minimal, zero-dependency Apple XML Plist parser.
pub fn parse_xml_plist(xml: &str) -> Result<PlistValue, SebError> {
    let plist_start = xml
        .find("<plist")
        .ok_or_else(|| SebError::XmlParse("missing <plist> root element".to_string()))?;
    let body_start = xml[plist_start..]
        .find('>')
        .map(|idx| plist_start + idx + 1)
        .ok_or_else(|| SebError::XmlParse("unterminated <plist> tag".to_string()))?;

    let mut parser = XmlCursor {
        src: xml,
        pos: body_start,
    };
    parser.parse_value()
}

struct XmlCursor<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> XmlCursor<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            let trimmed = self.rest().trim_start();
            self.pos = self.src.len() - trimmed.len();
            if self.rest().starts_with("<!--")
                && let Some(end) = self.rest().find("-->")
            {
                self.pos += end + 3;
                continue;
            }
            break;
        }
    }

    fn read_tag(&mut self) -> Result<(String, bool, bool), SebError> {
        self.skip_ws_and_comments();
        if !self.rest().starts_with('<') {
            return Err(SebError::XmlParse(format!(
                "expected '<' at byte {}",
                self.pos
            )));
        }
        let close = self
            .rest()
            .find('>')
            .ok_or_else(|| SebError::XmlParse("unclosed XML tag".to_string()))?;
        let raw = &self.rest()[1..close];
        self.pos += close + 1;

        let is_closing = raw.starts_with('/');
        let is_self_closing = raw.ends_with('/');
        let name = raw
            .trim_start_matches('/')
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();
        Ok((name, is_closing, is_self_closing))
    }

    fn read_text_until_close(&mut self, tag: &str) -> Result<String, SebError> {
        let needle = format!("</{tag}>");
        let end = self
            .rest()
            .find(&needle)
            .ok_or_else(|| SebError::XmlParse(format!("missing </{tag}> closing tag")))?;
        let text = &self.rest()[..end];
        self.pos += end + needle.len();
        Ok(unescape_xml(text))
    }

    fn parse_value(&mut self) -> Result<PlistValue, SebError> {
        let (tag, is_closing, is_self_closing) = self.read_tag()?;
        if is_closing {
            return Err(SebError::XmlParse(format!("unexpected closing </{tag}>")));
        }
        match tag.as_str() {
            "true" => Ok(PlistValue::Bool(true)),
            "false" => Ok(PlistValue::Bool(false)),
            "string" => {
                if is_self_closing {
                    Ok(PlistValue::String(String::new()))
                } else {
                    Ok(PlistValue::String(self.read_text_until_close("string")?))
                }
            }
            "data" | "date" => {
                if is_self_closing {
                    Ok(PlistValue::Data(String::new()))
                } else {
                    let raw = self.read_text_until_close(&tag)?;
                    let compact: String = raw.split_whitespace().collect();
                    Ok(PlistValue::Data(compact))
                }
            }
            "integer" => {
                let text = self.read_text_until_close("integer")?;
                let n = text
                    .trim()
                    .parse::<i64>()
                    .map_err(|_| SebError::XmlParse(format!("invalid integer '{text}'")))?;
                Ok(PlistValue::Integer(n))
            }
            "real" => {
                let text = self.read_text_until_close("real")?;
                let r = text
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| SebError::XmlParse(format!("invalid real '{text}'")))?;
                Ok(PlistValue::Real(r))
            }
            "dict" => {
                let mut map = BTreeMap::new();
                if is_self_closing {
                    return Ok(PlistValue::Dict(map));
                }
                loop {
                    self.skip_ws_and_comments();
                    if self.rest().starts_with("</dict>") {
                        self.pos += "</dict>".len();
                        break;
                    }
                    let (ktag, kclose, kself) = self.read_tag()?;
                    if kclose || ktag != "key" {
                        return Err(SebError::XmlParse(format!(
                            "expected <key> inside <dict>, got '{ktag}'"
                        )));
                    }
                    let key = if kself {
                        String::new()
                    } else {
                        self.read_text_until_close("key")?
                    };
                    let val = self.parse_value()?;
                    map.insert(key, val);
                }
                Ok(PlistValue::Dict(map))
            }
            "array" => {
                let mut items = Vec::new();
                if is_self_closing {
                    return Ok(PlistValue::Array(items));
                }
                loop {
                    self.skip_ws_and_comments();
                    if self.rest().starts_with("</array>") {
                        self.pos += "</array>".len();
                        break;
                    }
                    items.push(self.parse_value()?);
                }
                Ok(PlistValue::Array(items))
            }
            other => Err(SebError::XmlParse(format!("unsupported plist tag <{other}>"))),
        }
    }
}

fn unescape_xml(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::password::verify_password;

    const SAMPLE_SEB_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>originatorVersion</key>
    <string>SEB_OSX_3.4</string>
    <key>startURL</key>
    <string>https://canvas.school.edu/courses/10/quizzes/5?mode=exam&amp;v=1</string>
    <key>hashedQuitPassword</key>
    <string>a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3</string>
    <key>allowBrowsingBackForward</key>
    <true/>
    <key>browserWindowAllowReload</key>
    <false/>
    <key>allowSpellCheck</key>
    <true/>
    <key>allowVirtualMachine</key>
    <false/>
    <key>URLFilterRules</key>
    <array>
        <dict>
            <key>active</key>
            <true/>
            <key>action</key>
            <integer>1</integer>
            <key>expression</key>
            <string>*.instructure.com</string>
        </dict>
        <dict>
            <key>active</key>
            <true/>
            <key>action</key>
            <integer>0</integer>
            <key>expression</key>
            <string>chat.instructure.com</string>
        </dict>
    </array>
</dict>
</plist>"#;

    #[test]
    fn parses_plain_xml_seb_and_verifies_quit_password() {
        let doc = parse_seb_bytes(SAMPLE_SEB_XML.as_bytes()).expect("parse .seb");
        assert_eq!(
            doc.config.start_url.as_str(),
            "https://canvas.school.edu/courses/10/quizzes/5?mode=exam&v=1"
        );
        assert!(doc.config.lockdown.allow_back_forward);
        assert!(!doc.config.lockdown.allow_reload);
        assert!(doc.config.accessibility.allow_spell_check);
        assert!(doc.config.security.block_vm);
        assert_eq!(doc.config.allowed_domains, vec!["*.instructure.com"]);
        assert_eq!(doc.config.blocked_domains, vec!["chat.instructure.com"]);

        // SHA-256("123") == a665a459...
        let hash = doc.config.quit_password_hash.as_deref().unwrap();
        assert!(verify_password("123", hash).unwrap());
        assert!(!verify_password("wrong", hash).unwrap());

        assert_eq!(doc.seb_config_key.len(), 64);
        let req_hash =
            compute_seb_request_hash("https://canvas.school.edu/quiz", &doc.seb_config_key);
        assert_eq!(req_hash.len(), 64);
    }

    #[test]
    fn rejects_encrypted_seb_headers() {
        assert!(matches!(
            parse_seb_bytes(b"pswd1234567890"),
            Err(SebError::EncryptedUnsupported(_))
        ));
        assert!(matches!(
            parse_seb_bytes(b"pwcc1234567890"),
            Err(SebError::EncryptedUnsupported(_))
        ));
    }
}
