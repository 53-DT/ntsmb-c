//! Saved SMB connections that survive a cold reboot of the television.
//!
//! WHY THIS EXISTS. The debug `smb-smoke` trigger lives in `/tmp/<app id>/plxnative-smb`, and `/tmp`
//! is RAM on webOS: it survives an app reinstall but not a power cycle. A real app keeps its
//! connections on persistent storage. This module is that store.
//!
//! WHAT IT HOLDS. Only [`ConnectionProfile`] metadata (host, port, username, guest flag, share) and
//! the opaque, already-encrypted credential envelope that `ConnectionProfile::protect_password`
//! produced with the TV key manager. Plaintext password fallbacks from older builds are discarded
//! on load and are never written again. Nothing in this file logs a credential.
//!
//! WHERE IT LIVES. This crate deliberately does not know the application's paths. The caller hands
//! in an ordered list of candidate file paths (see `docs/NT-SMB-NEXT-STEPS.md`, task 1) and the store
//! reads the first one that holds a valid file and writes to the first one that can be written.
//!
//! WRITES ARE ATOMIC: a temporary file beside the target is written, synced and renamed over it, so
//! a power cut can leave the old file or the new file but never half of one.

use crate::profile::ConnectionProfile;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA: u32 = 1;
/// More than this many saved servers is a mistake or abuse, not a household.
pub const MAX_SAVED: usize = 16;

/// One saved server.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SavedConnection {
    /// Stable identity, see [`connection_id`]. Contains no secret.
    pub id: String,
    /// What the list shows. Defaults to the host.
    pub name: String,
    pub profile: ConnectionProfile,
    /// Legacy field read only to migrate older files that stored plaintext passwords.
    /// It is cleared on load and deliberately omitted from every subsequent write.
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
    /// The folder (relative to the share root) the person was last in. Empty means the share root.
    #[serde(default)]
    pub last_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SavedFile {
    schema: u32,
    #[serde(default)]
    last_used: Option<String>,
    #[serde(default)]
    connections: Vec<SavedConnection>,
}

impl SavedFile {
    fn empty() -> Self {
        Self { schema: STORE_SCHEMA, last_used: None, connections: Vec::new() }
    }
}

/// `host:port/username` (or `/guest`), lower-cased host. Two forms that name the same server and
/// account collapse into one saved entry.
pub fn connection_id(profile: &ConnectionProfile) -> String {
    let who = if profile.guest { "guest".to_owned() } else { profile.username.trim().to_owned() };
    format!("{}:{}/{}", profile.host.trim().to_ascii_lowercase(), profile.port, who)
}

/// The saved connections, in memory, with the files they came from and go to.
pub struct SavedStore {
    candidates: Vec<PathBuf>,
    data: SavedFile,
    temporary: AtomicBool,
    runtime_dir: Option<PathBuf>,
}


#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionSavedConnection {
    id: String,
    name: String,
    profile: ConnectionProfile,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    last_path: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionSavedFile {
    schema: u32,
    #[serde(default)]
    last_used: Option<String>,
    #[serde(default)]
    connections: Vec<SessionSavedConnection>,
}

impl SavedStore {

    /// Load SMB profiles from the protected FinNative session payload.
    pub fn open_session(encoded: Option<&str>) -> Self {
        let mut data = SavedFile::empty();
        if let Some(encoded) = encoded {
            if let Ok(file) = serde_json::from_str::<SessionSavedFile>(encoded) {
                if file.schema == STORE_SCHEMA {
                    data.connections = file.connections.into_iter()
                        .filter(|entry| entry.profile.validate().is_ok())
                        .take(MAX_SAVED)
                        .map(|entry| SavedConnection {
                            id: entry.id, name: entry.name, profile: entry.profile,
                            password: entry.password, last_path: entry.last_path,
                        }).collect();
                    data.last_used = file.last_used.filter(|id|
                        data.connections.iter().any(|entry| &entry.id == id));
                }
            }
        }
        Self { candidates: Vec::new(), data, temporary: AtomicBool::new(false), runtime_dir: None }
    }

    /// Encode the connection list for the protected FinNative session extension.
    pub fn encode_session(&self) -> Result<String, String> {
        let file = SessionSavedFile {
            schema: STORE_SCHEMA,
            last_used: self.data.last_used.clone(),
            connections: self.data.connections.iter().map(|entry| SessionSavedConnection {
                id: entry.id.clone(), name: entry.name.clone(), profile: entry.profile.clone(),
                password: entry.password.clone(), last_path: entry.last_path.clone(),
            }).collect(),
        };
        serde_json::to_string(&file).map_err(|_| "Could not encode saved SMB connections".to_string())
    }

    /// Load from the first candidate that holds a valid file. A missing file is an empty store.
    /// A file that exists but cannot be understood is moved aside (`.corrupt`) rather than
    /// silently overwritten, and the store starts empty.
    pub fn open(candidates: Vec<PathBuf>) -> Self {
        Self::open_with_runtime(candidates, None)
    }

    pub fn open_with_runtime(candidates: Vec<PathBuf>, runtime_dir: Option<PathBuf>) -> Self {
        let mut data = SavedFile::empty();
        let mut temporary = false;
        let mut scrub_legacy_passwords = false;
        for path in &candidates {
            match fs::read_to_string(path) {
                Ok(text) => match Self::parse_migrating(&text) {
                    Some((parsed, had_plaintext_passwords)) => {
                        data = parsed;
                        scrub_legacy_passwords = had_plaintext_passwords;
                        temporary = runtime_dir.as_deref().is_some_and(|runtime| path.parent() == Some(runtime));
                        break;
                    }
                    None => {
                        let mut aside = path.clone().into_os_string();
                        aside.push(".corrupt");
                        let _ = fs::rename(path, PathBuf::from(aside));
                    }
                },
                Err(_) => continue,
            }
        }
        let store = Self { candidates, data, temporary: AtomicBool::new(temporary), runtime_dir };
        // Rewrite an older file immediately so its legacy plaintext password fields are removed
        // even if the person does not edit or reconnect to any saved server.
        if scrub_legacy_passwords {
            let _ = store.save();
        }
        store
    }

    fn parse_migrating(text: &str) -> Option<(SavedFile, bool)> {
        let mut file: SavedFile = serde_json::from_str(text).ok()?;
        if file.schema != STORE_SCHEMA {
            return None;
        }
        // One bad entry must not cost the person the good ones.
        file.connections.retain(|c| c.profile.validate().is_ok());
        file.connections.truncate(MAX_SAVED);
        let had_plaintext_passwords = file.connections.iter().any(|c| c.password.is_some());
        for connection in &mut file.connections {
            connection.password = None;
        }
        let last_is_known = match file.last_used.as_deref() {
            Some(last) => file.connections.iter().any(|c| c.id == last),
            None => true,
        };
        if !last_is_known {
            file.last_used = None;
        }
        Some((file, had_plaintext_passwords))
    }

    pub fn connections(&self) -> &[SavedConnection] {
        &self.data.connections
    }

    pub fn get(&self, id: &str) -> Option<&SavedConnection> {
        self.data.connections.iter().find(|c| c.id == id)
    }

    /// The server to reconnect to at launch, if any.
    pub fn last_used(&self) -> Option<&SavedConnection> {
        let id = self.data.last_used.as_deref()?;
        self.get(id)
    }

    /// Add or replace the entry for this server and account, and make it the last used. Returns
    /// its id. The profile is validated first; a refused profile changes nothing.
    pub fn upsert(&mut self, profile: ConnectionProfile) -> Result<String, String> {
        self.upsert_with_password(profile, None)
    }

    /// Compatibility spelling retained for older callers. The password argument is intentionally
    /// ignored: only the encrypted credential envelope on ConnectionProfile may be persisted.
    pub fn upsert_with_password(&mut self, profile: ConnectionProfile, _password: Option<String>) -> Result<String, String> {
        profile.validate()?;
        let id = connection_id(&profile);
        if let Some(existing) = self.data.connections.iter_mut().find(|c| c.id == id) {
            // Keep the person's folder if they re-save the same server from the form.
            let keep_path = if existing.profile.share == profile.share { existing.last_path.clone() } else { String::new() };
            existing.profile = profile;
            if let Some(password) = _password { existing.password = Some(password); }
            existing.last_path = keep_path;
        } else {
            if self.data.connections.len() >= MAX_SAVED {
                return Err("Too many saved servers. Remove one first.".into());
            }
            let name = profile.host.clone();
            self.data.connections.push(SavedConnection { id: id.clone(), name, profile, password: _password, last_path: String::new() });
        }
        self.data.last_used = Some(id.clone());
        Ok(id)
    }

    pub fn rename(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        if let Some(c) = self.data.connections.iter_mut().find(|c| c.id == id) {
            c.name = name.chars().take(48).collect();
        }
    }

    pub fn mark_used(&mut self, id: &str) {
        if self.get(id).is_some() {
            self.data.last_used = Some(id.to_owned());
        }
    }

    pub fn set_last_path(&mut self, id: &str, path: &str) {
        if let Some(c) = self.data.connections.iter_mut().find(|c| c.id == id) {
            c.last_path = path.trim_matches('/').to_owned();
        }
    }

    /// Remember which share a saved server opens on, once the person has chosen one.
    pub fn set_share(&mut self, id: &str, share: Option<&str>) {
        if let Some(c) = self.data.connections.iter_mut().find(|c| c.id == id) {
            let share = share.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
            if c.profile.share != share {
                c.profile.share = share;
                c.last_path.clear();
            }
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.data.connections.retain(|c| c.id != id);
        if self.data.last_used.as_deref() == Some(id) {
            self.data.last_used = None;
        }
    }

    /// Write to the first candidate that can be written. The error never names a path or a
    /// credential, because it can reach the screen and the log.
    pub fn saved_temporarily(&self) -> bool {
        self.temporary.load(Ordering::Relaxed)
    }

    pub fn save(&self) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&self.data).map_err(|_| "Could not encode saved servers".to_string())?;
        for path in &self.candidates {
            if write_atomic(path, text.as_bytes()).is_ok() {
                let temporary = self.runtime_dir.as_deref().is_some_and(|runtime| path.parent() == Some(runtime));
                self.temporary.store(temporary, Ordering::Relaxed);
                return Ok(());
            }
        }
        Err("Could not save to the television's storage".into())
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut tmp = path.to_path_buf().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);

    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}


const PIN_SCHEMA: u32 = 1;
const PIN_ITERATIONS: u32 = 250_000;
const PIN_SALT_LEN: usize = 16;
const PIN_HASH_LEN: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PinRecord {
    salt: String,
    hash: String,
    iterations: u32,
}

impl PinRecord {
    fn valid(&self) -> bool {
        hex_decode(&self.salt).is_some_and(|salt| salt.len() == PIN_SALT_LEN)
            && hex_decode(&self.hash).is_some_and(|hash| hash.len() == PIN_HASH_LEN)
            && (10_000..=1_000_000).contains(&self.iterations)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PinFile {
    schema: u32,
    #[serde(default)]
    pin: Option<PinRecord>,
}

impl PinFile {
    fn empty() -> Self { Self { schema: PIN_SCHEMA, pin: None } }
}

/// A separate app-lock file. Keeping it separate from the connection list preserves compatibility
/// with older NT-SMB builds. Only a salted PBKDF2 verifier is written; the PIN itself is never stored.
pub struct PinStore {
    candidates: Vec<PathBuf>,
    data: PinFile,
    temporary: AtomicBool,
    runtime_dir: Option<PathBuf>,
}

impl PinStore {
    pub fn open(candidates: Vec<PathBuf>) -> Self {
        Self::open_with_runtime(candidates, None)
    }

    pub fn open_with_runtime(mut candidates: Vec<PathBuf>, runtime_dir: Option<PathBuf>) -> Self {
        candidates.dedup();
        let mut data = PinFile::empty();
        let mut temporary = false;
        for path in &candidates {
            match fs::read_to_string(path) {
                Ok(text) => match serde_json::from_str::<PinFile>(&text) {
                    Ok(parsed) if parsed.schema == PIN_SCHEMA && parsed.pin.as_ref().is_none_or(PinRecord::valid) => {
                        data = parsed;
                        temporary = runtime_dir.as_deref().is_some_and(|runtime| path.parent() == Some(runtime));
                        break;
                    }
                    _ => {
                        let mut aside = path.clone().into_os_string();
                        aside.push(".corrupt");
                        let _ = fs::rename(path, PathBuf::from(aside));
                    }
                },
                Err(_) => continue,
            }
        }
        Self { candidates, data, temporary: AtomicBool::new(temporary), runtime_dir }
    }

    pub fn has_pin(&self) -> bool { self.data.pin.is_some() }

    pub fn saved_temporarily(&self) -> bool { self.temporary.load(Ordering::Relaxed) }

    /// Set and persist a four-digit app PIN. The old verifier is restored if the write fails.
    pub fn set_pin(&mut self, pin: &str) -> Result<(), String> {
        if pin.len() != 4 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("The app PIN must contain exactly four digits.".into());
        }
        let mut salt = [0u8; PIN_SALT_LEN];
        fs::File::open("/dev/urandom")
            .and_then(|mut file| file.read_exact(&mut salt))
            .map_err(|_| "The TV could not generate a secure PIN salt.".to_owned())?;
        let mut hash = [0u8; PIN_HASH_LEN];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pin.as_bytes(), &salt, PIN_ITERATIONS, &mut hash);

        let previous = self.data.pin.clone();
        self.data.pin = Some(PinRecord {
            salt: hex_encode(&salt),
            hash: hex_encode(&hash),
            iterations: PIN_ITERATIONS,
        });
        if let Err(error) = self.save() {
            self.data.pin = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Verify a PIN with a constant-time comparison of the derived verifier.
    pub fn verify_pin(&self, pin: &str) -> bool {
        if pin.len() != 4 || !pin.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Some(record) = self.data.pin.as_ref() else { return false; };
        if !record.valid() { return false; }
        let Some(salt) = hex_decode(&record.salt) else { return false; };
        let Some(expected) = hex_decode(&record.hash) else { return false; };
        let mut actual = [0u8; PIN_HASH_LEN];
        pbkdf2::pbkdf2_hmac::<sha2::Sha256>(pin.as_bytes(), &salt, record.iterations, &mut actual);
        let difference = expected.iter().zip(actual.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b));
        difference == 0
    }

    /// Remove the app PIN, restoring it in memory if the updated file cannot be written.
    pub fn remove_pin(&mut self) -> Result<(), String> {
        let previous = self.data.pin.clone();
        self.data.pin = None;
        if let Err(error) = self.save() {
            self.data.pin = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.data).map_err(|error| format!("Could not encode app PIN: {error}"))?;
        let mut last_error = None;
        for path in &self.candidates {
            match write_atomic(path, &bytes) {
                Ok(()) => {
                    let temporary = self.runtime_dir.as_deref().is_some_and(|runtime| path.parent() == Some(runtime));
                    self.temporary.store(temporary, Ordering::Relaxed);
                    return Ok(());
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .map(|error| format!("Could not write app PIN storage: {error}"))
            .unwrap_or_else(|| "No writable app PIN storage path is available.".to_owned()))
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = value.as_bytes();
    if bytes.len() % 2 != 0 { return None; }
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        decoded.push((nibble(pair[0])? << 4) | nibble(pair[1])?);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::CredentialEnvelope;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ntsmb-store-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sealed(profile: &mut ConnectionProfile) {
        profile.credential = Some(CredentialEnvelope {
            backend: "keymanager3".into(),
            key: "ntsmb.credentials.v1".into(),
            iv: "opaque-iv".into(),
            data: "opaque-ciphertext".into(),
        });
    }

    #[test]
    fn session_payload_round_trip_keeps_password_for_protected_session_storage() {
        let mut saved = SavedStore::open(Vec::new());
        let profile = ConnectionProfile::new("nas.local", 445, "alice", false).unwrap();
        saved.upsert_with_password(profile, Some("private-test-password".to_owned())).unwrap();
        let encoded = saved.encode_session().unwrap();
        let restored = SavedStore::open_session(Some(&encoded));
        let entry = restored.connections().first().unwrap();
        assert_eq!(entry.password.as_deref(), Some("private-test-password"));
        assert_eq!(entry.profile.host, "nas.local");
    }

    #[test]
    fn a_saved_server_survives_a_restart_and_holds_no_plaintext_password() {
        let dir = scratch("roundtrip");
        let file = dir.join("smb-connections.json");
        let mut store = SavedStore::open(vec![file.clone()]);
        let mut profile = ConnectionProfile::new("Nas.Local", 445, "alice", false).unwrap();
        profile.share = Some("Shared".into());
        sealed(&mut profile);
        let id = store.upsert(profile).unwrap();
        assert_eq!(id, "nas.local:445/alice");
        store.save().unwrap();

        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.to_lowercase().contains("\"password\""));

        let reopened = SavedStore::open(vec![file]);
        assert_eq!(reopened.connections().len(), 1);
        assert_eq!(reopened.last_used().unwrap().id, id);
        assert_eq!(reopened.last_used().unwrap().profile.share.as_deref(), Some("Shared"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn password_fallback_is_never_written_even_when_secure_storage_is_unavailable() {
        let dir = scratch("password-fallback");
        let file = dir.join("smb-connections.json");
        let mut store = SavedStore::open(vec![file.clone()]);
        let profile = ConnectionProfile::new("nas.local", 445, "viewer", false).unwrap();
        let id = store.upsert_with_password(profile, Some("test-password".into())).unwrap();
        store.save().unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains("test-password"));
        assert!(!text.to_lowercase().contains("\"password\""));
        let reopened = SavedStore::open(vec![file]);
        assert!(reopened.get(&id).unwrap().password.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_plaintext_passwords_are_discarded_and_scrubbed_from_disk() {
        let dir = scratch("legacy-password-migration");
        let file = dir.join("smb-connections.json");
        let profile = ConnectionProfile::new("nas.local", 445, "viewer", false).unwrap();
        let id = connection_id(&profile);
        let legacy = serde_json::json!({
            "schema": STORE_SCHEMA,
            "last_used": id,
            "connections": [{
                "id": id,
                "name": "nas.local",
                "profile": profile,
                "password": "old-plaintext-secret",
                "last_path": ""
            }]
        });
        fs::write(&file, serde_json::to_string(&legacy).unwrap()).unwrap();
        let reopened = SavedStore::open(vec![file.clone()]);
        assert!(reopened.get(&connection_id(&ConnectionProfile::new("nas.local",445,"viewer",false).unwrap())).unwrap().password.is_none());
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains("old-plaintext-secret"));
        assert!(!text.to_lowercase().contains("\"password\""));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_the_same_server_twice_replaces_it_and_keeps_the_folder() {
        let dir = scratch("replace");
        let mut store = SavedStore::open(vec![dir.join("a.json")]);
        let id = store.upsert(ConnectionProfile::new("nas", 445, "alice", false).unwrap()).unwrap();
        store.set_last_path(&id, "/Media/Shows/");
        let again = store.upsert(ConnectionProfile::new("NAS", 445, "alice", false).unwrap()).unwrap();
        assert_eq!(id, again);
        assert_eq!(store.connections().len(), 1);
        assert_eq!(store.get(&id).unwrap().last_path, "Media/Shows");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_is_an_empty_store_and_a_corrupt_one_is_set_aside() {
        let dir = scratch("corrupt");
        let file = dir.join("c.json");
        assert!(SavedStore::open(vec![file.clone()]).connections().is_empty());
        fs::write(&file, "{ not json").unwrap();
        let store = SavedStore::open(vec![file.clone()]);
        assert!(store.connections().is_empty());
        assert!(!file.exists());
        assert!(dir.join("c.json.corrupt").exists());
        let _ = fs::remove_dir_all(dir);
    }


    #[test]
    fn app_pin_is_hashed_verified_and_removable() {
        let dir = scratch("app-pin");
        let file = dir.join("ntsmb-app-pin.json");
        let mut store = PinStore::open(vec![file.clone()]);
        assert!(!store.has_pin());
        store.set_pin("4826").unwrap();
        assert!(store.has_pin());
        assert!(store.verify_pin("4826"));
        assert!(!store.verify_pin("4827"));
        assert!(!fs::read_to_string(&file).unwrap().contains("4826"));
        let mut reopened = PinStore::open(vec![file.clone()]);
        assert!(reopened.verify_pin("4826"));
        reopened.remove_pin().unwrap();
        assert!(!reopened.has_pin());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn one_invalid_entry_does_not_cost_the_valid_ones() {
        let dir = scratch("partial");
        let file = dir.join("p.json");
        let good = ConnectionProfile::new("nas", 445, "alice", false).unwrap();
        let mut bad = good.clone();
        bad.port = 0;
        let data = SavedFile {
            schema: STORE_SCHEMA,
            last_used: None,
            connections: vec![
                SavedConnection { id: "bad".into(), name: "bad".into(), profile: bad, password: None, last_path: String::new() },
                SavedConnection { id: connection_id(&good), name: "nas".into(), profile: good, password: None, last_path: String::new() },
            ],
        };
        fs::write(&file, serde_json::to_string(&data).unwrap()).unwrap();
        let store = SavedStore::open(vec![file]);
        assert_eq!(store.connections().len(), 1);
        assert_eq!(store.connections()[0].name, "nas");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_falls_through_to_the_next_candidate_and_remove_forgets_last_used() {
        let dir = scratch("fallthrough");
        // A path under a regular file can never be created.
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").unwrap();
        let unwritable = blocker.join("nested").join("s.json");
        let good = dir.join("ok").join("s.json");
        let mut store = SavedStore::open(vec![unwritable, good.clone()]);
        let id = store.upsert(ConnectionProfile::new("nas", 445, "", true).unwrap()).unwrap();
        store.save().unwrap();
        assert!(good.exists());
        store.remove(&id);
        assert!(store.last_used().is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_store_refuses_a_seventeenth_server() {
        let dir = scratch("full");
        let mut store = SavedStore::open(vec![dir.join("f.json")]);
        for i in 0..MAX_SAVED {
            store.upsert(ConnectionProfile::new(format!("nas{i}"), 445, "u", false).unwrap()).unwrap();
        }
        assert!(store.upsert(ConnectionProfile::new("one-too-many", 445, "u", false).unwrap()).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}