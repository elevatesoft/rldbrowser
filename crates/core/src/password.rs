use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use thiserror::Error;

const PREFIX: &str = "sha256";
const SEB_PREFIX: &str = "seb_sha256";
const SALT_LEN: usize = 16;
const ITERATIONS: usize = 100_000;

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("failed to generate random salt: {0}")]
    Random(String),
    #[error("invalid password hash format")]
    InvalidFormat,
}

/// Generates a salted, iterated SHA-256 hash formatted as `sha256$<salt_hex>$<digest_hex>`.
pub fn hash_password(password: &str) -> Result<String, PasswordError> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| PasswordError::Random(e.to_string()))?;
    Ok(hash_password_with_salt(password, &salt))
}

/// Deterministic helper for testing and reproducible demo configs.
pub fn hash_password_with_salt(password: &str, salt: &[u8]) -> String {
    let digest = stretch(password.as_bytes(), salt);
    format!("{PREFIX}${}${}", hex::encode(salt), hex::encode(digest))
}

/// Verifies `candidate` against either:
/// - `sha256$<salt_hex>$<digest_hex>` (rustbrowser salted iterated SHA-256), or
/// - `seb_sha256$<digest_hex>` (Safe Exam Browser raw SHA-256 hex quit password).
pub fn verify_password(candidate: &str, encoded: &str) -> Result<bool, PasswordError> {
    if let Some(hex_digest) = encoded.strip_prefix(&format!("{SEB_PREFIX}$")) {
        let expected = parse_hex_32(hex_digest)?;
        let actual: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        return Ok(bool::from(actual.ct_eq(&expected)));
    }
    let (salt, expected) = parse_encoded(encoded)?;
    let actual = stretch(candidate.as_bytes(), &salt);
    Ok(bool::from(actual.ct_eq(&expected)))
}

pub(crate) fn is_valid_hash_format(encoded: &str) -> bool {
    if let Some(hex_digest) = encoded.strip_prefix(&format!("{SEB_PREFIX}$")) {
        return parse_hex_32(hex_digest).is_ok();
    }
    parse_encoded(encoded).is_ok()
}

fn parse_hex_32(hex_str: &str) -> Result<[u8; 32], PasswordError> {
    let bytes = hex::decode(hex_str).map_err(|_| PasswordError::InvalidFormat)?;
    bytes.try_into().map_err(|_| PasswordError::InvalidFormat)
}

fn parse_encoded(encoded: &str) -> Result<(Vec<u8>, [u8; 32]), PasswordError> {
    let mut parts = encoded.split('$');
    let prefix = parts.next().ok_or(PasswordError::InvalidFormat)?;
    let salt_hex = parts.next().ok_or(PasswordError::InvalidFormat)?;
    let digest_hex = parts.next().ok_or(PasswordError::InvalidFormat)?;
    if prefix != PREFIX || parts.next().is_some() {
        return Err(PasswordError::InvalidFormat);
    }

    let salt = hex::decode(salt_hex).map_err(|_| PasswordError::InvalidFormat)?;
    if salt.is_empty() {
        return Err(PasswordError::InvalidFormat);
    }

    let digest_vec = hex::decode(digest_hex).map_err(|_| PasswordError::InvalidFormat)?;
    let digest: [u8; 32] = digest_vec
        .try_into()
        .map_err(|_| PasswordError::InvalidFormat)?;

    Ok((salt, digest))
}

fn stretch(password: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(password);
    let mut state: [u8; 32] = hasher.finalize().into();

    for _ in 1..ITERATIONS {
        let mut round = Sha256::new();
        round.update(salt);
        round.update(state);
        state = round.finalize().into();
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let encoded = hash_password("correct-horse-battery").expect("hash");
        assert!(verify_password("correct-horse-battery", &encoded).unwrap());
        assert!(!verify_password("wrong-password", &encoded).unwrap());
    }

    #[test]
    fn rejects_invalid_format() {
        assert!(verify_password("abc", "not-a-hash").is_err());
        assert!(verify_password("abc", "sha256$zzzz$00").is_err());
    }
}
