//! DeckFiles' neutral source vault and application-wide preferences.
//!
//! This module belongs to plx_os: it stores provider-neutral records and uses only the TV
//! secure-store contract. It deliberately has no dependency on plx_smb or any UI crate.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const SOURCES_FILE: &str = "deckfiles_sources.json";
const SETTINGS_FILE: &str = "deckfiles_settings.json";
const FORMAT_VERSION: u32 = 1;
const MAX_SOURCES: usize = 64;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind { #[default] Smb, Local, GoogleDriveSlot, WebDavSlot }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SourceRecord {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
    #[serde(default)] pub host: String,
    #[serde(default = "default_port")] pub port: u16,
    #[serde(default)] pub share: String,
    #[serde(default)] pub username: String,
    #[serde(default)] pub guest: bool,
    #[serde(default)] pub has_saved_credential: bool,
    #[serde(default)] pub root_path: String,
    #[serde(default)] pub last_path: String,
    pub created_at: u64,
    #[serde(default)] pub last_connected_at: u64,
}
fn default_port() -> u16 { 445 }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum SkipInterval { Five, Ten, Fifteen, Thirty }
impl Default for SkipInterval { fn default() -> Self { Self::Ten } }
impl SkipInterval { pub fn seconds(self) -> u32 { match self { Self::Five=>5, Self::Ten=>10, Self::Fifteen=>15, Self::Thirty=>30 } } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum HdrOverlayBrightness { Fifty, SeventyFive, OneHundred }
impl Default for HdrOverlayBrightness { fn default() -> Self { Self::OneHundred } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum HudAutoHide { Three, Five, Eight }
impl Default for HudAutoHide { fn default() -> Self { Self::Five } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum SubtitlesMode { Always, ForeignAudio, ForcedOnly, Off }
impl Default for SubtitlesMode { fn default() -> Self { Self::Always } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ForcedSubtitles { PreferNonForced, PreferForced, OnlyForced }
impl Default for ForcedSubtitles { fn default() -> Self { Self::PreferNonForced } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum SubtitleSize { Small, Normal, Large, ExtraLarge }
impl Default for SubtitleSize { fn default() -> Self { Self::Large } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum SubtitlePosition { Low, Medium, High }
impl Default for SubtitlePosition { fn default() -> Self { Self::Low } }

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum SubtitleColor { White, Yellow, Amber, Gray }
impl Default for SubtitleColor { fn default() -> Self { Self::White } }

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct PlaybackSettings {
    pub skip_interval_sec: SkipInterval,
    pub instant_dpad_seeking: bool,
    pub hdr_overlay_brightness: HdrOverlayBrightness,
    pub hud_show_remaining: bool,
    pub hud_auto_hide_sec: HudAutoHide,
    pub hud_compact: bool,
    pub continuously_play_next: bool,
}
impl Default for PlaybackSettings {
    fn default() -> Self { Self { skip_interval_sec: SkipInterval::Ten, instant_dpad_seeking: true,
        hdr_overlay_brightness: HdrOverlayBrightness::OneHundred, hud_show_remaining: true,
        hud_auto_hide_sec: HudAutoHide::Five, hud_compact: false, continuously_play_next: true } }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct AudioSubtitleSettings {
    pub preferred_audio_lang: String,
    pub subtitles_mode: SubtitlesMode,
    pub subtitle_lang: String,
    pub forced_subtitles: ForcedSubtitles,
    pub subtitle_size: SubtitleSize,
    pub subtitle_position: SubtitlePosition,
    pub subtitle_color: SubtitleColor,
}
impl Default for AudioSubtitleSettings {
    fn default() -> Self { Self { preferred_audio_lang: "original".into(), subtitles_mode: SubtitlesMode::Always,
        subtitle_lang: "eng".into(), forced_subtitles: ForcedSubtitles::PreferNonForced,
        subtitle_size: SubtitleSize::Large, subtitle_position: SubtitlePosition::Low,
        subtitle_color: SubtitleColor::White } }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct RecoverySettings {
    pub auto_recover_video_errors: bool,
    pub prefer_remux_containers: bool,
    pub sandbox_plane_rebind: bool,
}
impl Default for RecoverySettings {
    fn default() -> Self { Self { auto_recover_video_errors: true, prefer_remux_containers: true, sandbox_plane_rebind: true } }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct DeckSettings {
    pub playback: PlaybackSettings,
    pub audio_subtitles: AudioSubtitleSettings,
    pub recovery: RecoverySettings,
}

#[derive(Clone, Deserialize, Serialize)]
struct CredentialRecord {
    sealed: crate::tv::secure::Sealed,
}
impl PartialEq for CredentialRecord {
    fn eq(&self, other: &Self) -> bool { self.sealed == other.sealed }
}
impl Eq for CredentialRecord {}
impl std::fmt::Debug for CredentialRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("CredentialRecord { <sealed> }") }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct VaultFile {
    format: u32,
    sources: Vec<SourceRecord>,
    credentials: BTreeMap<String, CredentialRecord>,
}
impl Default for VaultFile {
    fn default() -> Self { Self { format: FORMAT_VERSION, sources: Vec::new(), credentials: BTreeMap::new() } }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SettingsFile { format: u32, settings: DeckSettings }

/// One process-local view of DeckFiles' provider-neutral metadata and app-wide preferences.
pub struct DeckVault {
    sources_path: PathBuf,
    settings_path: PathBuf,
    data: VaultFile,
    settings: DeckSettings,
}

impl DeckVault {
    pub fn load(app_data_dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = app_data_dir.as_ref();
        fs::create_dir_all(dir).map_err(|_| "Could not open DeckFiles storage".to_string())?;
        let sources_path = dir.join(SOURCES_FILE);
        let settings_path = dir.join(SETTINGS_FILE);
        let data = match fs::read(&sources_path) {
            Ok(bytes) => {
                let parsed: VaultFile = serde_json::from_slice(&bytes).map_err(|_| "DeckFiles source storage is invalid".to_string())?;
                if parsed.format != FORMAT_VERSION || parsed.sources.len() > MAX_SOURCES {
                    return Err("Unsupported DeckFiles source storage version".into());
                }
                parsed
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => VaultFile::default(),
            Err(_) => return Err("Could not read DeckFiles source storage".into()),
        };
        let settings = match fs::read(&settings_path) {
            Ok(bytes) => {
                let parsed: SettingsFile = serde_json::from_slice(&bytes).map_err(|_| "DeckFiles settings are invalid".to_string())?;
                if parsed.format != FORMAT_VERSION { return Err("Unsupported DeckFiles settings version".into()); }
                parsed.settings
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DeckSettings::default(),
            Err(_) => return Err("Could not read DeckFiles settings".into()),
        };
        let vault = Self { sources_path, settings_path, data, settings };
        if !vault.sources_path.exists() { vault.persist_sources()?; }
        if !vault.settings_path.exists() { vault.persist_settings()?; }
        Ok(vault)
    }

    pub fn list_sources(&self) -> &[SourceRecord] { &self.data.sources }
    pub fn load_settings(&self) -> DeckSettings { self.settings.clone() }

    pub fn save_settings(&mut self, settings: DeckSettings) -> Result<(), String> {
        validate_settings(&settings)?;
        self.settings = settings;
        self.persist_settings()
    }

    pub fn upsert_source(&mut self, mut record: SourceRecord, optional_secret: Option<&str>) -> Result<(), String> {
        if record.id.trim().is_empty() || record.label.trim().is_empty() { return Err("Source identity and label are required".into()); }
        if let Some(secret) = optional_secret {
            if secret.is_empty() { return Err("Credential cannot be empty".into()); }
            let sealed = crate::tv::secure::seal(secret.as_bytes()).ok_or_else(|| "The device could not securely protect this credential".to_string())?;
            self.data.credentials.insert(record.id.clone(), CredentialRecord { sealed });
            record.has_saved_credential = true;
        } else if self.data.credentials.contains_key(&record.id) {
            record.has_saved_credential = true;
        } else {
            record.has_saved_credential = false;
        }
        if let Some(existing) = self.data.sources.iter_mut().find(|source| source.id == record.id) {
            record.created_at = existing.created_at;
            *existing = record;
        } else {
            if self.data.sources.len() >= MAX_SOURCES { return Err("Too many saved sources".into()); }
            self.data.sources.push(record);
        }
        self.persist_sources()
    }

    pub fn unseal_secret(&self, id: &str) -> Option<String> {
        let sealed = &self.data.credentials.get(id)?.sealed;
        let bytes = crate::tv::secure::open(sealed)?;
        String::from_utf8(bytes).ok()
    }

    pub fn clear_credential(&mut self, id: &str) -> Result<(), String> {
        if let Some(record) = self.data.credentials.remove(id) {
            crate::tv::secure::remove(&record.sealed.backend, &record.sealed.key);
        }
        if let Some(source) = self.data.sources.iter_mut().find(|source| source.id == id) {
            source.has_saved_credential = false;
        }
        self.persist_sources()
    }

    pub fn remove_source(&mut self, id: &str) -> Result<(), String> {
        self.data.sources.retain(|source| source.id != id);
        if let Some(record) = self.data.credentials.remove(id) {
            crate::tv::secure::remove(&record.sealed.backend, &record.sealed.key);
        }
        self.persist_sources()
    }

    fn persist_sources(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&self.data).map_err(|_| "Could not encode DeckFiles source storage".to_string())?;
        atomic_write(&self.sources_path, &bytes, SOURCES_FILE)
    }

    fn persist_settings(&self) -> Result<(), String> {
        let value = SettingsFile { format: FORMAT_VERSION, settings: self.settings.clone() };
        let bytes = serde_json::to_vec_pretty(&value).map_err(|_| "Could not encode DeckFiles settings".to_string())?;
        atomic_write(&self.settings_path, &bytes, SETTINGS_FILE)
    }
}

fn atomic_write(path: &Path, bytes: &[u8], file_name: &str) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "Invalid DeckFiles storage path".to_string())?;
    let temp = parent.join(format!(".{file_name}.tmp"));
    let mut file = OpenOptions::new().create(true).truncate(true).write(true).open(&temp)
        .map_err(|_| "Could not write DeckFiles storage".to_string())?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|_| "Could not write DeckFiles storage".to_string())?;
    drop(file);
    fs::rename(&temp, path).map_err(|_| "Could not commit DeckFiles storage".to_string())?;
    if let Ok(dir) = OpenOptions::new().read(true).open(parent) { let _ = dir.sync_all(); }
    Ok(())
}

fn validate_settings(settings: &DeckSettings) -> Result<(), String> {
    const LANGS: &[&str] = &["original", "eng", "jpn", "spa", "fre", "ger", "kor", "hin"];
    if !LANGS.contains(&settings.audio_subtitles.preferred_audio_lang.as_str()) {
        return Err("Unsupported preferred audio language".into());
    }
    if !["eng", "jpn", "spa", "fre", "ger", "kor", "hin"].contains(&settings.audio_subtitles.subtitle_lang.as_str()) {
        return Err("Unsupported subtitle language".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_deckfiles_preferences() {
        let settings = DeckSettings::default();
        assert_eq!(settings.playback.skip_interval_sec.seconds(), 10);
        assert!(settings.playback.instant_dpad_seeking);
        assert_eq!(settings.audio_subtitles.preferred_audio_lang, "original");
        assert_eq!(settings.audio_subtitles.subtitle_lang, "eng");
        assert!(settings.recovery.auto_recover_video_errors);
    }
    #[test]
    fn settings_round_trip_and_invalid_language_is_refused() {
        let encoded = serde_json::to_string(&DeckSettings::default()).unwrap();
        let decoded: DeckSettings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, DeckSettings::default());
        let mut invalid = DeckSettings::default();
        invalid.audio_subtitles.preferred_audio_lang = "secret".into();
        assert!(validate_settings(&invalid).is_err());
    }
}
