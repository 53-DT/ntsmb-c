//! Fresh DeckFiles application composition.
//!
//! This crate owns the provider registry, neutral vault and DeckFiles screen state. It deliberately
//! has no dependency on the legacy app, Plex/Jellyfin clients, appkit, or legacy screen registry.
//! Provider calls perform blocking I/O and must be invoked by the platform's worker path, not draw.

use deckfiles_screens::add_source_modal::AddSourceModal;
use deckfiles_screens::deck_player::{DeckPlayerScreen, PlayerSnapshot};
use deckfiles_screens::deck_settings::{
    CredentialItem, DeckSettingsDraft, DeckSettingsScreen, SettingsRoot,
};
use deckfiles_screens::files_browser::{
    FileItem, FileKind, FilesBrowserScreen, SourceItem,
};
use plx_os::storage::deck_vault::{
    AudioSubtitleSettings, DeckSettings, DeckVault, ForcedSubtitles, HdrOverlayBrightness,
    HudAutoHide, RecoverySettings, SkipInterval, SourceKind, SubtitleColor, SubtitlePosition,
    SubtitleSize, SubtitlesMode,
};
use plx_smb::provider::{
    ConnectPayload, PreparedSourcePlay, SourceRegistry,
};
use plx_ui::{Painter, Rect};
pub mod remote_control;
use remote_control::{FocusZone, RemoteControl};

/// Which DeckFiles surface is currently visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeckFilesPage { #[default] Files, AddSource, Settings, Player }

/// Platform-neutral input vocabulary for the DeckFiles remote-control adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeckFilesInput {
    Up, Down, Left, Right, Confirm, Back, NextField, ToggleMode,
    Text(String), Backspace, ToggleGuest, CycleSource(isize), OpenFiles, OpenAddSource, OpenSettings,
}

/// Work requested by the UI. The host must execute I/O away from the render thread.
#[derive(Clone, Debug)]
pub enum DeckFilesAction {
    None,
    ListDirectory { source_id: String, path: String },
    Connect(ConnectPayload),
    PreparePlayback { source_id: String, path: String },
    SaveSettings,
}
/// The state shared by the Files, Add Source, Settings and Player surfaces.
pub struct DeckFilesApp {
    pub page: DeckFilesPage,
    pub vault: DeckVault,
    providers: SourceRegistry,
    pub files: FilesBrowserScreen,
    pub add_source: AddSourceModal,
    pub settings: DeckSettingsScreen,
    pub player: DeckPlayerScreen,
    pub remote: RemoteControl,
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
            page: DeckFilesPage::Files,
            providers: SourceRegistry::new(),
            files: FilesBrowserScreen::new(),
            add_source: AddSourceModal::new(),
            settings: DeckSettingsScreen::new(settings, credentials),
            player: DeckPlayerScreen::new(PlayerSnapshot::default()),
            remote: RemoteControl::default(),
        };
        app.refresh_sources();
        app
    }

    /// Draw the currently selected DeckFiles surface.
    pub fn draw(&self, painter: Painter, bounds: Rect) {
        match self.page {
            DeckFilesPage::Files => self.files.draw_with_focus(painter, bounds, if self.remote.zone()==FocusZone::Sidebar {Some(self.remote.sidebar_index())} else {None}),
            DeckFilesPage::AddSource => self.add_source.draw(painter, bounds),
            DeckFilesPage::Settings => self.settings.draw(painter, bounds),
            DeckFilesPage::Player => self.player.draw(painter, bounds),
        }
    }

    /// Reload the vault after an I/O worker has persisted a source.
    pub fn reload_vault(&mut self, app_data_dir: impl AsRef<std::path::Path>) -> Result<(), String> {
        self.vault = DeckVault::load(app_data_dir)?;
        self.refresh_sources();
        Ok(())
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

    /// Apply one input edge and return any I/O action for the host adapter.
    /// No network or directory enumeration runs in this method.
    pub fn handle_input(&mut self, input: DeckFilesInput) -> DeckFilesAction {
        use deckfiles_screens::add_source_modal::SourceProviderChoice;
        match input {
            DeckFilesInput::OpenFiles => { self.page = DeckFilesPage::Files; self.remote.focus_content(); },
            DeckFilesInput::OpenAddSource => { self.add_source.cancel(); self.page = DeckFilesPage::AddSource; }
            DeckFilesInput::OpenSettings => { self.reload_settings(); self.page = DeckFilesPage::Settings; }
            DeckFilesInput::CycleSource(delta) if self.page == DeckFilesPage::Files => {
                let available = self.files.sources.iter().filter(|source| source.available).collect::<Vec<_>>();
                if !available.is_empty() {
                    let current = self.files.active_source.as_deref()
                        .and_then(|id| available.iter().position(|source| source.id == id));
                    let next = match current {
                        Some(index) => (index as isize + delta).rem_euclid(available.len() as isize) as usize,
                        None if delta < 0 => available.len() - 1,
                        None => 0,
                    };
                    let source_id = available[next].id.clone();
                    if self.files.choose_source(&source_id) {
                        self.files.set_loading(true);
                        return DeckFilesAction::ListDirectory { source_id, path: String::new() };
                    }
                }
            }
            DeckFilesInput::Up => match self.page {
                DeckFilesPage::Files => if self.remote.zone()==FocusZone::Sidebar { self.remote.move_sidebar(-1,3+self.files.sources.len()); } else { self.files.move_selection(-1); },
                DeckFilesPage::AddSource => self.add_source.move_focus(-1),
                DeckFilesPage::Settings => self.settings.move_row(-1),
                DeckFilesPage::Player => { let _=self.player.handle_key(deckfiles_screens::deck_player::PlayerKey::Up); }
            },
            DeckFilesInput::Down => match self.page {
                DeckFilesPage::Files => if self.remote.zone()==FocusZone::Sidebar { self.remote.move_sidebar(1,3+self.files.sources.len()); } else { self.files.move_selection(1); },
                DeckFilesPage::AddSource => self.add_source.move_focus(1),
                DeckFilesPage::Settings => self.settings.move_row(1),
                DeckFilesPage::Player => { let _=self.player.handle_key(deckfiles_screens::deck_player::PlayerKey::Down); }
            },
            DeckFilesInput::Left if self.page == DeckFilesPage::AddSource => self.add_source.move_focus(-1),
            DeckFilesInput::Left if self.page == DeckFilesPage::Files && self.remote.zone()==FocusZone::Sidebar => self.remote.focus_content(),
            DeckFilesInput::Left if self.page == DeckFilesPage::Files && self.files.current_path.is_empty() => self.remote.focus_sidebar(),
            DeckFilesInput::Left if self.page == DeckFilesPage::Settings => { let i=SettingsRoot::ALL.iter().position(|r|*r==self.settings.selected_root).unwrap_or(0); self.settings.select_root(SettingsRoot::ALL[i.saturating_sub(1)]); },
            DeckFilesInput::Right | DeckFilesInput::NextField if self.page == DeckFilesPage::AddSource => self.add_source.move_focus(1),
            DeckFilesInput::Right if self.page == DeckFilesPage::Files && self.remote.zone()==FocusZone::Sidebar => self.remote.focus_content(),
            DeckFilesInput::Right if self.page == DeckFilesPage::Settings => { let i=SettingsRoot::ALL.iter().position(|r|*r==self.settings.selected_root).unwrap_or(0); self.settings.select_root(SettingsRoot::ALL[(i+1).min(SettingsRoot::ALL.len()-1)]); },
            DeckFilesInput::Left if self.page == DeckFilesPage::Files && !self.files.current_path.is_empty() => {
                if let (Some(source_id), Some(path)) = (self.files.active_source.clone(), self.files.go_back()) {
                    self.files.set_loading(true);
                    return DeckFilesAction::ListDirectory { source_id, path };
                }
            }
            DeckFilesInput::Back if self.page == DeckFilesPage::Files => {
                if let (Some(source_id), Some(path)) = (self.files.active_source.clone(), self.files.go_back()) {
                    self.files.set_loading(true);
                    return DeckFilesAction::ListDirectory { source_id, path };
                }
            }
            DeckFilesInput::ToggleGuest if self.page == DeckFilesPage::AddSource => {
                self.add_source.draft.guest = !self.add_source.draft.guest;
                if self.add_source.draft.guest { self.add_source.draft.password.clear(); }
                self.add_source.error = None;
                self.add_source.focused_field = self.add_source.focused_field.min(self.add_source.field_count().saturating_sub(1));
            }
            DeckFilesInput::ToggleMode if self.page == DeckFilesPage::Files => self.files.toggle_mode(),
            DeckFilesInput::Text(value) if self.page == DeckFilesPage::AddSource => {
                let field = self.add_source.focused_field;
                let target = match self.add_source.draft.provider {
                    SourceProviderChoice::Smb => match field {
                        0 => Some(&mut self.add_source.draft.label), 1 => Some(&mut self.add_source.draft.host),
                        3 => Some(&mut self.add_source.draft.share), 4 => Some(&mut self.add_source.draft.username),
                        5 => Some(&mut self.add_source.draft.password), _ => None,
                    },
                    SourceProviderChoice::Local => match field {
                        0 => Some(&mut self.add_source.draft.label), 1 => Some(&mut self.add_source.draft.local_folder), _ => None,
                    },
                    SourceProviderChoice::GoogleDrive => None,
                };
                if let Some(target) = target { if !(self.add_source.draft.provider == SourceProviderChoice::Smb && self.add_source.draft.guest && field == 5) { target.push_str(&value); } }
                if self.add_source.draft.provider == SourceProviderChoice::Smb && field == 2 {
                    let mut port = self.add_source.draft.port.to_string(); port.push_str(&value);
                    if let Ok(parsed) = port.parse::<u16>() { self.add_source.draft.port = parsed; }
                }
                self.add_source.error = None;
            }
            DeckFilesInput::Backspace if self.page == DeckFilesPage::AddSource => {
                let field = self.add_source.focused_field;
                if self.add_source.draft.provider == SourceProviderChoice::Smb && field == 2 {
                    let mut port = self.add_source.draft.port.to_string();
                    port.pop();
                    if port.is_empty() { port = "0".into(); }
                    if let Ok(parsed) = port.parse::<u16>() { self.add_source.draft.port = parsed; }
                }
                let target = match self.add_source.draft.provider {
                    SourceProviderChoice::Smb => match field {
                        0 => Some(&mut self.add_source.draft.label), 1 => Some(&mut self.add_source.draft.host),
                        3 => Some(&mut self.add_source.draft.share), 4 => Some(&mut self.add_source.draft.username),
                        5 => Some(&mut self.add_source.draft.password), _ => None,
                    },
                    SourceProviderChoice::Local => match field {
                        0 => Some(&mut self.add_source.draft.label), 1 => Some(&mut self.add_source.draft.local_folder), _ => None,
                    },
                    SourceProviderChoice::GoogleDrive => None,
                };
                if let Some(target) = target { if !(self.add_source.draft.provider == SourceProviderChoice::Smb && self.add_source.draft.guest && field == 5) { target.pop(); } }
            }
            DeckFilesInput::Confirm => match self.page {
                DeckFilesPage::Files if self.remote.zone()==FocusZone::Sidebar => {
                    match self.remote.sidebar_index() {
                        0 => self.remote.focus_content(),
                        1 => { self.reload_settings(); self.page=DeckFilesPage::Settings; },
                        2 => { self.add_source.cancel(); self.page=DeckFilesPage::AddSource; self.add_source.focused_field=0; },
                        index => if let Some(source)=self.files.sources.get(index-3).filter(|s|s.available).cloned() { if self.files.choose_source(&source.id) { self.files.set_loading(true); return DeckFilesAction::ListDirectory{source_id:source.id,path:String::new()}; } },
                    }
                },
                DeckFilesPage::Files => if let Some(entry) = self.files.selected_entry().cloned() {
                    if let Some(source_id) = self.files.active_source.clone() {
                        if entry.is_folder() {
                            if let Some(path) = self.files.enter_selected() {
                                self.files.set_loading(true);
                                return DeckFilesAction::ListDirectory { source_id, path };
                            }
                        } else if entry.is_media() {
                            return DeckFilesAction::PreparePlayback { source_id, path: entry.path };
                        }
                    }
                },
                DeckFilesPage::AddSource => {
                    match self.add_source.focused_field {
                        6 if self.add_source.draft.provider == SourceProviderChoice::Smb => { self.add_source.draft.guest = !self.add_source.draft.guest; if self.add_source.draft.guest { self.add_source.draft.password.clear(); self.add_source.draft.save_password=false; } self.add_source.error=None; },
                        7 if self.add_source.draft.provider == SourceProviderChoice::Smb => { if !self.add_source.draft.guest { self.add_source.draft.save_password = !self.add_source.draft.save_password; } },
                        index if index == self.add_source.field_count().saturating_sub(1) => if let Ok(draft) = self.add_source.begin_connect() {
                    let kind = match draft.provider {
                        SourceProviderChoice::Smb => SourceKind::Smb,
                        SourceProviderChoice::Local => SourceKind::Local,
                        SourceProviderChoice::GoogleDrive => SourceKind::GoogleDriveSlot,
                    };
                    return DeckFilesAction::Connect(ConnectPayload {
                        kind, label: draft.label, host: draft.host, port: draft.port, share: draft.share,
                        username: draft.username, guest: draft.guest, save_password: draft.save_password,
                        secret: (!draft.guest).then_some(draft.password), root_path: draft.local_folder,
                    });
                        }, _ => {}
                    }
                },
                DeckFilesPage::Settings => return DeckFilesAction::SaveSettings,
                DeckFilesPage::Player => {},
            },
            DeckFilesInput::Back => match self.page {
                DeckFilesPage::AddSource => { self.add_source.cancel(); self.page = DeckFilesPage::Files; }
                DeckFilesPage::Settings | DeckFilesPage::Player => { self.page = DeckFilesPage::Files; self.remote.focus_content(); },
                DeckFilesPage::Files => {},
            },
            _ => {},
        }
        DeckFilesAction::None
    }

    /// Publish the result of an asynchronous connection attempt.
    pub fn finish_connect(&mut self, result: Result<String, String>) {
        match result {
            Ok(_) => { self.add_source.finish_connect(Ok(())); self.page = DeckFilesPage::Files; self.refresh_sources(); }
            Err(error) => self.add_source.finish_connect(Err(error)),
        }
    }

    /// Publish a directory result returned by the host's I/O worker.
    pub fn finish_directory(&mut self, result: Result<Vec<plx_smb::provider::MediaEntry>, String>) {
        match result {
            Ok(entries) => self.files.set_entries(entries.into_iter().map(|entry| FileItem {
                name: entry.name, path: entry.path,
                kind: match entry.kind {
                    plx_smb::provider::EntryKind::Folder => FileKind::Folder,
                    plx_smb::provider::EntryKind::Video => FileKind::Video,
                    plx_smb::provider::EntryKind::Subtitle => FileKind::Subtitle,
                    plx_smb::provider::EntryKind::Other => FileKind::Other,
                }, size_bytes: entry.size_bytes,
            }).collect()),
            Err(error) => self.files.set_error(Some(error)),
        }
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
