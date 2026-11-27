//! Fresh DeckFiles application composition.
//!
//! This crate owns the provider registry, neutral vault and DeckFiles screen state. It deliberately
//! has no dependency on the legacy app, Plex/Jellyfin clients, appkit, or legacy screen registry.
//! Provider calls perform blocking I/O and must be invoked by the platform's worker path, not draw.

use plx_deckfiles_screens::add_source_modal::AddSourceModal;
use plx_deckfiles_screens::deck_player::{DeckPlayerScreen, PlayerSnapshot};
use plx_deckfiles_screens::deck_settings::{
    CredentialItem, DeckSettingsDraft, DeckSettingsScreen, SettingsRoot,
};
use plx_deckfiles_screens::files_browser::{
    FileItem, FileKind, FilesBrowserScreen, SourceItem,
};
use plx_os::storage::deck_vault::{
    AudioSubtitleSettings, DeckSettings, DeckVault, ForcedSubtitles, HdrOverlayBrightness,
    HudAutoHide, RecoverySettings, SkipInterval, SourceKind, SubtitleColor, SubtitlePosition,
    SubtitleSize, SubtitlesMode,
};
use plx_smb::provider::{
    ConnectPayload, PreparedSourcePlay, SourceProvider, SourceRegistry,
};

/// The state shared by the Files, Add Source, Settings and Player surfaces.
pub struct DeckFilesApp {
    pub vault: DeckVault,
    providers: SourceRegistry,
    pub files: FilesBrowserScreen,
    pub add_source: AddSourceModal,
    pub settings: DeckSettingsScreen,
    pub player: DeckPlayerScreen,
}

impl DeckFilesApp {
    /// Load the independent DeckFiles JSON vault from the platform's application data directory.
    pub fn load(app_data_dir: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let vault = DeckVault::load(app_data_dir)?;
        Ok(Self::new(vault))
    }

    pub fn new(vault: DeckVault) -> Self {
        let settings = settings_draft(&vault.load_settings());
        let credentials = credential_items(&vault);
        let mut app = Self {
            vault,
            providers: SourceRegistry::new(),
            files: FilesBrowserScreen::new(),
            add_source: AddSourceModal::new(),
            settings: DeckSettingsScreen::new(settings, credentials),
            player: DeckPlayerScreen::new(PlayerSnapshot::default()),
        };
        app.refresh_sources();
        app
    }

    /// Refresh the source rail and the credentials page from the provider-neutral vault.
    pub fn refresh_sources(&mut self) {
        self.files.set_sources(self.vault.list_sources().iter().map(|source| {
            let provider = self.providers.get(source.kind);
            SourceItem {
                id: source.id.clone(),
                label: source.label.clone(),
                kind_label: provider.map(|p| p.display_name()).unwrap_or("Unavailable").to_string(),
                available: provider.is_some_and(|p| p.available()),
                has_saved_credential: source.has_saved_credential,
            }
        }).collect());
        self.settings.credentials = credential_items(&self.vault);
    }

    /// Connect and persist a source. Call from an I/O worker: SMB authentication is blocking.
    pub fn connect_source(&mut self, payload: &ConnectPayload) -> Result<String, String> {
        let provider = self.providers.get(payload.kind)
            .ok_or_else(|| "This source provider is unavailable".to_string())?;
        let source = provider.connect(payload, &mut self.vault)?;
        self.refresh_sources();
        Ok(source.id)
    }

    /// List one directory and publish its rows to the Files screen.
    /// Call from an I/O worker: SMB directory enumeration is blocking.
    pub fn list_directory(&mut self, source_id: &str, relative_path: &str) -> Result<(), String> {
        let source = self.vault.list_sources().iter().find(|s| s.id == source_id).cloned()
            .ok_or_else(|| "Saved source was not found".to_string())?;
        let provider = self.providers.get(source.kind)
            .filter(|p| p.available())
            .ok_or_else(|| "This source provider is unavailable".to_string())?;
        let entries = provider.list_dir(&source, relative_path, &self.vault)?;
        if self.files.active_source.as_deref() != Some(source_id) {
            if !self.files.choose_source(source_id) {
                return Err("This source cannot be opened".into());
            }
        }
        self.files.current_path = relative_path.to_string();
        self.files.set_entries(entries.into_iter().map(|entry| FileItem {
            name: entry.name,
            path: entry.path,
            kind: match entry.kind {
                plx_smb::provider::EntryKind::Folder => FileKind::Folder,
                plx_smb::provider::EntryKind::Video => FileKind::Video,
                plx_smb::provider::EntryKind::Subtitle => FileKind::Subtitle,
                plx_smb::provider::EntryKind::Other => FileKind::Other,
            },
            size_bytes: entry.size_bytes,
        }).collect());
        Ok(())
    }

    /// Prepare the selected file using the source provider's native playback preparation.
    /// Call from an I/O worker; the returned value is handed to the URI/feed playback adapter.
    pub fn prepare_play(&self, source_id: &str, file_path: &str) -> Result<PreparedSourcePlay, String> {
        let source = self.vault.list_sources().iter().find(|s| s.id == source_id)
            .ok_or_else(|| "Saved source was not found".to_string())?;
        let provider = self.providers.get(source.kind)
            .filter(|p| p.available())
            .ok_or_else(|| "This source provider is unavailable".to_string())?;
        provider.prepare_play(source, file_path, &self.vault)
    }

    pub fn remove_source(&mut self, source_id: &str) -> Result<(), String> {
        self.vault.remove_source(source_id)?;
        self.refresh_sources();
        Ok(())
    }

    /// Persist the current four-root Settings draft back to the OS-owned DeckFiles vault.
    pub fn save_settings(&mut self) -> Result<(), String> {
        let draft = &self.settings.settings;
        let mut saved = self.vault.load_settings();
        saved.playback.skip_interval_sec = match draft.skip_interval_seconds {
            5 => SkipInterval::Five, 10 => SkipInterval::Ten,
            15 => SkipInterval::Fifteen, 30 => SkipInterval::Thirty,
            _ => return Err("Unsupported skip interval".into()),
        };
        saved.playback.instant_dpad_seeking = draft.instant_dpad_seeking;
        saved.playback.hdr_overlay_brightness = match draft.hdr_overlay_brightness_percent {
            50 => HdrOverlayBrightness::Fifty, 75 => HdrOverlayBrightness::SeventyFive,
            100 => HdrOverlayBrightness::OneHundred,
            _ => return Err("Unsupported HDR overlay brightness".into()),
        };
        saved.playback.hud_show_remaining = draft.show_remaining_time;
        saved.playback.hud_auto_hide_sec = match draft.hud_auto_hide_seconds {
            3 => HudAutoHide::Three, 5 => HudAutoHide::Five, 8 => HudAutoHide::Eight,
            _ => return Err("Unsupported HUD auto-hide interval".into()),
        };
        saved.playback.hud_compact = draft.compact_hud;
        saved.playback.continuously_play_next = draft.continuously_play_next;
        saved.audio_subtitles = AudioSubtitleSettings {
            preferred_audio_lang: draft.preferred_audio_language.clone(),
            subtitles_mode: parse_subtitles_mode(&draft.subtitles_mode)?,
            subtitle_lang: draft.subtitle_language.clone(),
            forced_subtitles: parse_forced_subtitles(&draft.forced_subtitles)?,
            subtitle_size: parse_subtitle_size(&draft.subtitle_size)?,
            subtitle_position: parse_subtitle_position(&draft.subtitle_position)?,
            subtitle_color: parse_subtitle_color(&draft.subtitle_color)?,
        };
        saved.recovery = RecoverySettings {
            auto_recover_video_errors: draft.auto_recover_video_errors,
            prefer_remux_containers: draft.prefer_remux_containers,
            sandbox_plane_rebind: draft.sandbox_video_plane_rebind,
        };
        self.vault.save_settings(saved)
    }

    pub fn open_settings_root(&mut self, root: SettingsRoot) {
        self.settings.select_root(root);
    }

    pub fn reload_settings(&mut self) {
        self.settings = DeckSettingsScreen::new(
            settings_draft(&self.vault.load_settings()),
            credential_items(&self.vault),
        );
    }

    pub fn provider_available(&self, kind: SourceKind) -> bool {
        self.providers.get(kind).is_some_and(|provider| provider.available())
    }
}

fn credential_items(vault: &DeckVault) -> Vec<CredentialItem> {
    vault.list_sources().iter().map(|source| CredentialItem {
        source_id: source.id.clone(),
        label: source.label.clone(),
        has_saved_credential: source.has_saved_credential,
        guest: source.guest,
    }).collect()
}

fn settings_draft(settings: &DeckSettings) -> DeckSettingsDraft {
    DeckSettingsDraft {
        skip_interval_seconds: settings.playback.skip_interval_sec.seconds(),
        instant_dpad_seeking: settings.playback.instant_dpad_seeking,
        hdr_overlay_brightness_percent: match settings.playback.hdr_overlay_brightness {
            HdrOverlayBrightness::Fifty => 50,
            HdrOverlayBrightness::SeventyFive => 75,
            HdrOverlayBrightness::OneHundred => 100,
        },
        show_remaining_time: settings.playback.hud_show_remaining,
        hud_auto_hide_seconds: match settings.playback.hud_auto_hide_sec {
            HudAutoHide::Three => 3, HudAutoHide::Five => 5, HudAutoHide::Eight => 8,
        },
        compact_hud: settings.playback.hud_compact,
        continuously_play_next: settings.playback.continuously_play_next,
        preferred_audio_language: settings.audio_subtitles.preferred_audio_lang.clone(),
        subtitles_mode: format!("{:?}", settings.audio_subtitles.subtitles_mode),
        subtitle_language: settings.audio_subtitles.subtitle_lang.clone(),
        forced_subtitles: format!("{:?}", settings.audio_subtitles.forced_subtitles),
        subtitle_size: format!("{:?}", settings.audio_subtitles.subtitle_size),
        subtitle_position: format!("{:?}", settings.audio_subtitles.subtitle_position),
        subtitle_color: format!("{:?}", settings.audio_subtitles.subtitle_color),
        auto_recover_video_errors: settings.recovery.auto_recover_video_errors,
        prefer_remux_containers: settings.recovery.prefer_remux_containers,
        sandbox_video_plane_rebind: settings.recovery.sandbox_plane_rebind,
    }
}

fn parse_subtitles_mode(value: &str) -> Result<SubtitlesMode, String> {
    match value { "Always" => Ok(SubtitlesMode::Always), "ForeignAudio" => Ok(SubtitlesMode::ForeignAudio),
        "ForcedOnly" => Ok(SubtitlesMode::ForcedOnly), "Off" => Ok(SubtitlesMode::Off),
        _ => Err("Unsupported subtitles mode".into()) }
}
fn parse_forced_subtitles(value: &str) -> Result<ForcedSubtitles, String> {
    match value { "PreferNonForced" => Ok(ForcedSubtitles::PreferNonForced), "PreferForced" => Ok(ForcedSubtitles::PreferForced),
        "OnlyForced" => Ok(ForcedSubtitles::OnlyForced), _ => Err("Unsupported forced subtitle mode".into()) }
}
fn parse_subtitle_size(value: &str) -> Result<SubtitleSize, String> {
    match value { "Small" => Ok(SubtitleSize::Small), "Normal" => Ok(SubtitleSize::Normal),
        "Large" => Ok(SubtitleSize::Large), "ExtraLarge" => Ok(SubtitleSize::ExtraLarge),
        _ => Err("Unsupported subtitle size".into()) }
}
fn parse_subtitle_position(value: &str) -> Result<SubtitlePosition, String> {
    match value { "Low" => Ok(SubtitlePosition::Low), "Medium" => Ok(SubtitlePosition::Medium),
        "High" => Ok(SubtitlePosition::High), _ => Err("Unsupported subtitle position".into()) }
}
fn parse_subtitle_color(value: &str) -> Result<SubtitleColor, String> {
    match value { "White" => Ok(SubtitleColor::White), "Yellow" => Ok(SubtitleColor::Yellow),
        "Amber" => Ok(SubtitleColor::Amber), "Gray" => Ok(SubtitleColor::Gray),
        _ => Err("Unsupported subtitle color".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_preserves_every_saved_preference() {
        let mut settings = DeckSettings::default();
        settings.playback.skip_interval_sec = SkipInterval::Thirty;
        settings.playback.hdr_overlay_brightness = HdrOverlayBrightness::SeventyFive;
        settings.playback.hud_auto_hide_sec = HudAutoHide::Eight;
        settings.audio_subtitles.subtitles_mode = SubtitlesMode::ForeignAudio;
        settings.audio_subtitles.forced_subtitles = ForcedSubtitles::OnlyForced;
        settings.audio_subtitles.subtitle_size = SubtitleSize::ExtraLarge;
        settings.audio_subtitles.subtitle_position = SubtitlePosition::High;
        settings.audio_subtitles.subtitle_color = SubtitleColor::Amber;
        let draft = settings_draft(&settings);
        assert_eq!(draft.skip_interval_seconds, 30);
        assert_eq!(draft.hdr_overlay_brightness_percent, 75);
        assert_eq!(draft.hud_auto_hide_seconds, 8);
        assert_eq!(parse_subtitles_mode(&draft.subtitles_mode).unwrap(), SubtitlesMode::ForeignAudio);
        assert_eq!(parse_forced_subtitles(&draft.forced_subtitles).unwrap(), ForcedSubtitles::OnlyForced);
        assert_eq!(parse_subtitle_size(&draft.subtitle_size).unwrap(), SubtitleSize::ExtraLarge);
        assert_eq!(parse_subtitle_position(&draft.subtitle_position).unwrap(), SubtitlePosition::High);
        assert_eq!(parse_subtitle_color(&draft.subtitle_color).unwrap(), SubtitleColor::Amber);
    }

    #[test]
    fn source_kinds_are_provider_neutral_and_cloud_slots_stay_unavailable() {
        let app = DeckFilesApp::new(DeckVault::load(std::env::temp_dir().join(
            format!("deckfiles-app-test-{}", std::process::id())
        )).unwrap());
        assert!(!app.provider_available(SourceKind::GoogleDriveSlot));
        assert!(!app.provider_available(SourceKind::WebDavSlot));
    }
}
