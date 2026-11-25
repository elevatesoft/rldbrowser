use thiserror::Error;
use url::Url;

use crate::config::{ConfigError, ExamConfig};

pub const LAUNCH_SCHEME: &str = "rld";
pub const LEGACY_LAUNCH_SCHEME: &str = "rustbrowser";

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("invalid launch URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error(
        "unsupported launch scheme '{0}' (expected '{LAUNCH_SCHEME}' or '{LEGACY_LAUNCH_SCHEME}')"
    )]
    UnsupportedScheme(String),
    #[error("unsupported launch action '{0}' (expected 'open' or 'config')")]
    UnsupportedAction(String),
    #[error("missing required query parameter '{0}'")]
    MissingParameter(&'static str),
    #[error("invalid base64url payload in launch link")]
    InvalidBase64,
    #[error("launch payload is not valid UTF-8")]
    InvalidUtf8,
    #[error("invalid exam configuration: {0}")]
    Config(#[from] ConfigError),
}

/// Parses an `rld://` (or legacy `rustbrowser://`) launch URL into a validated [`ExamConfig`].
///
/// Supported formats:
/// - `rld://open?url=https%3A%2F%2Fexam.school.edu%2Fquiz&name=Midterm`
/// - `rld://config?data=<base64url-encoded-exam.toml>`
pub fn parse_launch_url(raw: &str) -> Result<ExamConfig, LaunchError> {
    let parsed = Url::parse(raw)?;
    if parsed.scheme() != LAUNCH_SCHEME && parsed.scheme() != LEGACY_LAUNCH_SCHEME {
        return Err(LaunchError::UnsupportedScheme(parsed.scheme().to_string()));
    }

    let action = parsed.host_str().unwrap_or("");
    match action {
        "open" => {
            let mut target_url: Option<String> = None;
            let mut exam_name: Option<String> = None;
            for (k, v) in parsed.query_pairs() {
                match k.as_ref() {
                    "url" => target_url = Some(v.into_owned()),
                    "name" => exam_name = Some(v.into_owned()),
                    _ => {}
                }
            }
            let raw_target = target_url.ok_or(LaunchError::MissingParameter("url"))?;
            let url = Url::parse(&raw_target)?;
            let mut config = ExamConfig::for_url(url)?;
            if let Some(name) = exam_name
                && !name.trim().is_empty()
            {
                config.exam_name = name.trim().to_string();
            }
            Ok(config)
        }
        "config" => {
            let data = parsed
                .query_pairs()
                .find_map(|(k, v)| (k == "data").then(|| v.into_owned()))
                .ok_or(LaunchError::MissingParameter("data"))?;
            let bytes = decode_base64url(&data).ok_or(LaunchError::InvalidBase64)?;
            let toml_str = String::from_utf8(bytes).map_err(|_| LaunchError::InvalidUtf8)?;
            Ok(ExamConfig::from_toml_str(&toml_str)?)
        }
        other => Err(LaunchError::UnsupportedAction(other.to_string())),
    }
}

/// Encodes a TOML exam config string into an `rld://config?data=...` launch link.
pub fn encode_config_launch_url(toml_str: &str) -> String {
    format!(
        "{LAUNCH_SCHEME}://config?data={}",
        encode_base64url(toml_str.as_bytes())
    )
}

fn encode_base64url(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3f) as usize] as char);
        }
    }
    out
}

fn decode_base64url(input: &str) -> Option<Vec<u8>> {
    let trimmed = input.trim_end_matches('=');
    let mut out = Vec::with_capacity((trimmed.len() * 3) / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;

    for byte in trimmed.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_launch_url() {
        let cfg = parse_launch_url(
            "rld://open?url=https%3A%2F%2Fcanvas.school.edu%2Fquiz%2F1&name=Bio%20Quiz",
        )
        .expect("valid open link");
        assert_eq!(cfg.exam_name, "Bio Quiz");
        assert_eq!(cfg.start_url.as_str(), "https://canvas.school.edu/quiz/1");
    }

    #[test]
    fn roundtrips_config_launch_url() {
        let toml = r#"
exam_name = "Chemistry Final"
start_url = "https://exam.school.edu/chem"
allowed_domains = ["*.school.edu"]
"#;
        let link = encode_config_launch_url(toml);
        assert!(link.starts_with("rld://config?data="));
        let cfg = parse_launch_url(&link).expect("valid config link");
        assert_eq!(cfg.exam_name, "Chemistry Final");
        assert_eq!(cfg.allowed_domains, vec!["*.school.edu"]);
    }

    #[test]
    fn accepts_legacy_rustbrowser_scheme() {
        let open_cfg = parse_launch_url(
            "rustbrowser://open?url=https%3A%2F%2Fcanvas.school.edu%2Fquiz%2F2&name=Legacy%20Quiz",
        )
        .expect("valid legacy open link");
        assert_eq!(open_cfg.exam_name, "Legacy Quiz");
        assert_eq!(
            open_cfg.start_url.as_str(),
            "https://canvas.school.edu/quiz/2"
        );

        let toml = r#"
exam_name = "Legacy Final"
start_url = "https://exam.school.edu/legacy"
allowed_domains = ["*.school.edu"]
"#;
        let legacy_link = encode_config_launch_url(toml).replacen(
            &format!("{LAUNCH_SCHEME}://"),
            &format!("{LEGACY_LAUNCH_SCHEME}://"),
            1,
        );
        assert!(legacy_link.starts_with("rustbrowser://config?data="));
        let config_cfg = parse_launch_url(&legacy_link).expect("valid legacy config link");
        assert_eq!(config_cfg.exam_name, "Legacy Final");
        assert_eq!(config_cfg.allowed_domains, vec!["*.school.edu"]);
    }

    #[test]
    fn rejects_invalid_scheme_and_http_target() {
        assert!(matches!(
            parse_launch_url("https://example.com"),
            Err(LaunchError::UnsupportedScheme(_))
        ));
        assert!(matches!(
            parse_launch_url("rld://open?url=http%3A%2F%2Fexample.com"),
            Err(LaunchError::Config(ConfigError::InvalidStartUrl { .. }))
        ));
    }
}
