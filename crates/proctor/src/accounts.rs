use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rustbrowser_core::{hash_password, verify_password};
use serde::{Deserialize, Serialize};

/// 32-character unambiguous alphabet (excludes `0`, `O`, `1`, `I`).
const ROOM_CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const MIN_PASSWORD_LEN: usize = 6;
const MIN_CUSTOM_CODE_LEN: usize = 3;
const MAX_CUSTOM_CODE_LEN: usize = 24;
const MAX_CODE_GEN_ATTEMPTS: usize = 64;

static ENTROPY_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeacherAccount {
    pub id: String,
    pub email: String,
    pub name: String,
    pub password_hash: String,
}

impl TeacherAccount {
    pub fn public_view(&self) -> TeacherPublicView {
        TeacherPublicView {
            id: self.id.clone(),
            email: self.email.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeacherPublicView {
    pub id: String,
    pub email: String,
    pub name: String,
}

impl From<&TeacherAccount> for TeacherPublicView {
    fn from(account: &TeacherAccount) -> Self {
        account.public_view()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub code: String,
    pub name: String,
    pub owner_id: String,
    pub created_at_epoch: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedStore {
    #[serde(default)]
    accounts: BTreeMap<String, TeacherAccount>,
    #[serde(default)]
    rooms: BTreeMap<String, RoomSummary>,
}

#[derive(Debug)]
pub struct AccountStore {
    accounts: BTreeMap<String, TeacherAccount>,
    sessions: BTreeMap<String, String>,
    rooms: BTreeMap<String, RoomSummary>,
    storage_path: Option<PathBuf>,
}

impl AccountStore {
    /// Creates a new `AccountStore`, loading persisted accounts and rooms from
    /// `storage_path` when present. Missing or unreadable files gracefully start empty.
    pub fn new(storage_path: Option<PathBuf>) -> Self {
        let mut accounts = BTreeMap::new();
        let mut rooms = BTreeMap::new();

        if let Some(ref path) = storage_path
            && let Ok(raw) = fs::read_to_string(path)
            && let Ok(persisted) = serde_json::from_str::<PersistedStore>(&raw)
        {
            for (_, mut account) in persisted.accounts {
                let key = account.email.trim().to_ascii_lowercase();
                account.email = key.clone();
                accounts.insert(key, account);
            }
            for (_, mut room) in persisted.rooms {
                let key = room.code.trim().to_ascii_uppercase();
                room.code = key.clone();
                rooms.insert(key, room);
            }
        }

        Self {
            accounts,
            sessions: BTreeMap::new(),
            rooms,
            storage_path,
        }
    }

    pub fn register(
        &mut self,
        name: &str,
        email: &str,
        password: &str,
    ) -> Result<(TeacherPublicView, String), String> {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return Err("Name cannot be empty".to_string());
        }

        let normalized_email = normalize_and_validate_email(email)?;

        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(format!(
                "Password must be at least {MIN_PASSWORD_LEN} characters"
            ));
        }

        if self.accounts.contains_key(&normalized_email) {
            return Err("An account with this email already exists".to_string());
        }

        let password_hash =
            hash_password(password).map_err(|e| format!("Failed to hash password: {e}"))?;

        let account = TeacherAccount {
            id: generate_teacher_id(),
            email: normalized_email.clone(),
            name: trimmed_name.to_string(),
            password_hash,
        };
        let public_view = account.public_view();

        self.accounts.insert(normalized_email.clone(), account);
        if let Err(err) = self.save() {
            self.accounts.remove(&normalized_email);
            return Err(err);
        }

        let token = self.mint_session(&normalized_email);
        Ok((public_view, token))
    }

    pub fn login(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<(TeacherPublicView, String), String> {
        let normalized_email = email.trim().to_ascii_lowercase();
        let Some(account) = self.accounts.get(&normalized_email) else {
            return Err("Invalid email or password".to_string());
        };

        let verified = verify_password(password, &account.password_hash).unwrap_or(false);
        if !verified {
            return Err("Invalid email or password".to_string());
        }

        let public_view = account.public_view();
        let token = self.mint_session(&normalized_email);
        Ok((public_view, token))
    }

    pub fn logout(&mut self, session_token: &str) {
        self.sessions.remove(session_token);
    }

    pub fn authenticate(&self, session_token: &str) -> Option<TeacherPublicView> {
        let email = self.sessions.get(session_token)?;
        self.accounts.get(email).map(TeacherAccount::public_view)
    }

    pub fn create_room(
        &mut self,
        owner_id: &str,
        name: &str,
        custom_code: Option<&str>,
    ) -> Result<RoomSummary, String> {
        let trimmed_owner = owner_id.trim();
        if trimmed_owner.is_empty() {
            return Err("Owner ID cannot be empty".to_string());
        }

        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return Err("Room name cannot be empty".to_string());
        }

        let code = match custom_code {
            Some(raw) => {
                let normalized = validate_custom_room_code(raw)?;
                if self.rooms.contains_key(&normalized) {
                    return Err(format!("Room code '{normalized}' already exists"));
                }
                normalized
            }
            None => self.generate_unique_room_code()?,
        };

        let summary = RoomSummary {
            code: code.clone(),
            name: trimmed_name.to_string(),
            owner_id: trimmed_owner.to_string(),
            created_at_epoch: current_epoch_secs(),
        };

        self.rooms.insert(code.clone(), summary.clone());
        if let Err(err) = self.save() {
            self.rooms.remove(&code);
            return Err(err);
        }

        Ok(summary)
    }

    pub fn list_rooms_for_owner(&self, owner_id: &str) -> Vec<RoomSummary> {
        let mut owned: Vec<RoomSummary> = self
            .rooms
            .values()
            .filter(|room| room.owner_id == owner_id)
            .cloned()
            .collect();
        owned.sort_by(|a, b| {
            b.created_at_epoch
                .cmp(&a.created_at_epoch)
                .then_with(|| a.code.cmp(&b.code))
        });
        owned
    }

    pub fn get_room(&self, code: &str) -> Option<&RoomSummary> {
        let normalized = code.trim().to_ascii_uppercase();
        self.rooms.get(&normalized)
    }

    pub fn delete_room(&mut self, owner_id: &str, code: &str) -> bool {
        let normalized = code.trim().to_ascii_uppercase();
        let is_owner = self
            .rooms
            .get(&normalized)
            .is_some_and(|room| room.owner_id == owner_id);
        if !is_owner {
            return false;
        }
        self.rooms.remove(&normalized);
        let _ = self.save();
        true
    }

    pub fn has_any_accounts(&self) -> bool {
        !self.accounts.is_empty()
    }

    fn mint_session(&mut self, email: &str) -> String {
        let token = generate_hex_token_128();
        self.sessions.insert(token.clone(), email.to_string());
        token
    }

    fn generate_unique_room_code(&self) -> Result<String, String> {
        for _ in 0..MAX_CODE_GEN_ATTEMPTS {
            let candidate = generate_room_code();
            if !self.rooms.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err("Failed to allocate a unique room code".to_string())
    }

    fn save(&self) -> Result<(), String> {
        let Some(ref path) = self.storage_path else {
            return Ok(());
        };

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create storage directory: {e}"))?;
        }

        let persisted = PersistedStore {
            accounts: self.accounts.clone(),
            rooms: self.rooms.clone(),
        };
        let payload = serde_json::to_string_pretty(&persisted)
            .map_err(|e| format!("Failed to serialize account store: {e}"))?;

        let seq = ENTROPY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("accounts.json");
        let tmp_path = path.with_file_name(format!("{file_name}.tmp.{}.{seq}", std::process::id()));

        fs::write(&tmp_path, &payload)
            .map_err(|e| format!("Failed to write temporary account store: {e}"))?;
        if let Err(rename_err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            fs::write(path, &payload).map_err(|write_err| {
                format!("Failed to persist account store ({rename_err}; fallback: {write_err})")
            })?;
        }

        Ok(())
    }
}

fn normalize_and_validate_email(email: &str) -> Result<String, String> {
    let trimmed = email.trim().to_ascii_lowercase();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return Err("Please enter a valid email address".to_string());
    }
    let Some((local, domain)) = trimmed.split_once('@') else {
        return Err("Please enter a valid email address".to_string());
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err("Please enter a valid email address".to_string());
    }
    Ok(trimmed)
}

fn validate_custom_room_code(raw: &str) -> Result<String, String> {
    let code = raw.trim().to_ascii_uppercase();
    let len = code.len();
    if !(MIN_CUSTOM_CODE_LEN..=MAX_CUSTOM_CODE_LEN).contains(&len) {
        return Err(format!(
            "Room code must be between {MIN_CUSTOM_CODE_LEN} and {MAX_CUSTOM_CODE_LEN} characters"
        ));
    }
    if code.starts_with('-')
        || code.ends_with('-')
        || !code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(
            "Room code may only contain letters, numbers, and interior hyphens".to_string(),
        );
    }
    Ok(code)
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fill_random_bytes(buf: &mut [u8]) {
    let filled_from_urandom = fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .is_ok();
    if filled_from_urandom {
        return;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = u64::from(std::process::id());
    let seq = ENTROPY_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut state = nanos ^ pid.rotate_left(17) ^ seq.wrapping_mul(0x9e37_79b9_7f4a_7c15);

    for chunk in buf.chunks_mut(8) {
        // SplitMix64 mixing step for fallback entropy when /dev/urandom is unavailable.
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        let bytes = z.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

fn generate_hex_token_128() -> String {
    let mut bytes = [0u8; 16];
    fill_random_bytes(&mut bytes);
    encode_hex(&bytes)
}

fn generate_teacher_id() -> String {
    let mut bytes = [0u8; 8];
    fill_random_bytes(&mut bytes);
    format!("tchr_{}", encode_hex(&bytes))
}

fn generate_room_code() -> String {
    let mut bytes = [0u8; 6];
    fill_random_bytes(&mut bytes);
    let mut code = String::with_capacity(7);
    for (idx, b) in bytes.into_iter().enumerate() {
        if idx == 3 {
            code.push('-');
        }
        code.push(ROOM_CODE_ALPHABET[(b & 31) as usize] as char);
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_path(label: &str) -> PathBuf {
        let seq = ENTROPY_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rustbrowser_accounts_test_{label}_{}_{seq}.json",
            std::process::id()
        ))
    }

    #[test]
    fn register_duplicate_login_auth_and_logout() {
        let mut store = AccountStore::new(None);
        assert!(!store.has_any_accounts());

        // Validation checks
        assert!(store.register("  ", "ada@school.edu", "secret1").is_err());
        assert!(store.register("Ada", "not-an-email", "secret1").is_err());
        assert!(store.register("Ada", "ada@school.edu", "short").is_err());

        // Successful registration
        let (teacher, reg_token) = store
            .register("Ada Lovelace", "Ada@School.EDU", "analytical1843")
            .expect("register succeeds");
        assert!(store.has_any_accounts());
        assert_eq!(teacher.email, "ada@school.edu");
        assert_eq!(teacher.name, "Ada Lovelace");
        assert_eq!(reg_token.len(), 32);
        assert!(reg_token.chars().all(|c| c.is_ascii_hexdigit()));

        // Duplicate email rejection (case-insensitive)
        assert!(
            store
                .register("Another Ada", "ADA@school.edu", "anotherpass")
                .is_err()
        );

        // Session authentication works
        assert_eq!(store.authenticate(&reg_token), Some(teacher.clone()));
        assert_eq!(store.authenticate("invalid-token"), None);

        // Login failure with wrong password or unknown email
        assert!(store.login("ada@school.edu", "wrong-password").is_err());
        assert!(store.login("unknown@school.edu", "analytical1843").is_err());

        // Login success mints a fresh valid session token
        let (logged_in, login_token) = store
            .login("  ADA@SCHOOL.EDU ", "analytical1843")
            .expect("login succeeds");
        assert_eq!(logged_in, teacher);
        assert_eq!(login_token.len(), 32);
        assert_ne!(login_token, reg_token);
        assert_eq!(store.authenticate(&login_token), Some(teacher));

        // Logout invalidates only that session token
        store.logout(&reg_token);
        assert_eq!(store.authenticate(&reg_token), None);
        assert!(store.authenticate(&login_token).is_some());

        store.logout(&login_token);
        assert_eq!(store.authenticate(&login_token), None);
    }

    #[test]
    fn room_creation_listing_and_deletion() {
        let mut store = AccountStore::new(None);
        let (teacher_a, _) = store
            .register("Teacher A", "a@school.edu", "password123")
            .expect("register A");
        let (teacher_b, _) = store
            .register("Teacher B", "b@school.edu", "password123")
            .expect("register B");

        // Auto-generated room code format: XXX-XXX without 0, O, 1, I
        let auto_room = store
            .create_room(&teacher_a.id, "AP CS Period 1", None)
            .expect("create auto room");
        assert_eq!(auto_room.code.len(), 7);
        assert_eq!(&auto_room.code[3..4], "-");
        for (idx, ch) in auto_room.code.chars().enumerate() {
            if idx == 3 {
                assert_eq!(ch, '-');
            } else {
                assert!(
                    !matches!(ch, '0' | 'O' | '1' | 'I'),
                    "ambiguous character {ch} in generated code {}",
                    auto_room.code
                );
                assert!(ROOM_CODE_ALPHABET.contains(&(ch as u8)));
            }
        }

        // Custom code normalization and duplicate rejection
        let custom_room = store
            .create_room(&teacher_a.id, "Calculus Midterm", Some("calc-2026"))
            .expect("create custom room");
        assert_eq!(custom_room.code, "CALC-2026");
        assert!(
            store
                .create_room(&teacher_b.id, "Duplicate", Some("CALC-2026"))
                .is_err()
        );

        let room_b = store
            .create_room(&teacher_b.id, "Biology Lab", Some("bio-101"))
            .expect("create room B");

        // Lookup is case-insensitive
        assert_eq!(store.get_room("calc-2026"), Some(&custom_room));
        assert_eq!(store.get_room("MISSING"), None);

        // Listing by owner isolates rooms per teacher
        let a_rooms = store.list_rooms_for_owner(&teacher_a.id);
        assert_eq!(a_rooms.len(), 2);
        assert!(a_rooms.iter().any(|r| r.code == auto_room.code));
        assert!(a_rooms.iter().any(|r| r.code == "CALC-2026"));

        let b_rooms = store.list_rooms_for_owner(&teacher_b.id);
        assert_eq!(b_rooms, vec![room_b.clone()]);

        // Deletion enforces ownership
        assert!(!store.delete_room(&teacher_b.id, "CALC-2026"));
        assert!(store.get_room("CALC-2026").is_some());

        assert!(store.delete_room(&teacher_a.id, "calc-2026"));
        assert!(store.get_room("CALC-2026").is_none());
        assert_eq!(store.list_rooms_for_owner(&teacher_a.id).len(), 1);
    }

    #[test]
    fn json_persistence_roundtrip() {
        let path = unique_temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        // Starts empty gracefully when file does not exist yet
        let mut store = AccountStore::new(Some(path.clone()));
        assert!(!store.has_any_accounts());

        let (teacher, _) = store
            .register("Grace Hopper", "grace@navy.mil", "cobol1959")
            .expect("register");
        let room = store
            .create_room(&teacher.id, "Compilers 101", Some("COMP-101"))
            .expect("create room");

        // Reload from disk into a fresh store
        let mut reloaded = AccountStore::new(Some(path.clone()));
        assert!(reloaded.has_any_accounts());

        let (logged_in, token) = reloaded
            .login("grace@navy.mil", "cobol1959")
            .expect("login after reload");
        assert_eq!(logged_in, teacher);
        assert_eq!(reloaded.authenticate(&token), Some(teacher.clone()));

        let reloaded_room = reloaded.get_room("COMP-101").expect("room persisted");
        assert_eq!(reloaded_room, &room);
        assert_eq!(reloaded.list_rooms_for_owner(&teacher.id), vec![room]);

        // Deleting a room persists the deletion
        assert!(reloaded.delete_room(&teacher.id, "COMP-101"));
        let reloaded_after_delete = AccountStore::new(Some(path.clone()));
        assert!(reloaded_after_delete.get_room("COMP-101").is_none());

        let _ = fs::remove_file(&path);
    }
}
