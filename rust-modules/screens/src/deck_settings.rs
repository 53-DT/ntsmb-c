//! DeckFiles' deliberately small application settings surface.
//!
//! Only the four DeckFiles roots are represented here. This screen has no Plex, Jellyfin,
//! library-management, account, telemetry, or system-settings vocabulary.

use plx_os::storage::deck_vault::{DeckVault, DeckSettings, SourceRecord, SourceKind};
use plx_ui::{Painter, Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsRoot { #[default] Playback, AudioSubtitles, Credentials, Recovery }

impl SettingsRoot {
    pub const ALL: [Self; 4] = [Self::Playback, Self::AudioSubtitles, Self::Credentials, Self::Recovery];
    pub fn title(self) -> &'static str {
        match self { Self::Playback => "Playback", Self::AudioSubtitles => "Audio & Subtitles",
            Self::Credentials => "Vault & Credentials", Self::Recovery => "Recovery" }
    }
}

pub struct DeckSettingsScreen {
    pub settings: DeckSettings,
    pub selected_root: SettingsRoot,
    pub selected_row: usize,
    pub sources: Vec<SourceRecord>,
    pub status: Option<String>,
}

impl Default for DeckSettingsScreen {
    fn default() -> Self { Self::new(DeckSettings::default(), &[]) }
}

impl DeckSettingsScreen {
    pub fn new(settings: DeckSettings, sources: &[SourceRecord]) -> Self {
        Self { settings, selected_root: SettingsRoot::Playback, selected_row: 0,
            sources: sources.to_vec(), status: None }
    }

    pub fn select_root(&mut self, root: SettingsRoot) {
        self.selected_root = root;
        self.selected_row = 0;
        self.status = None;
    }

    pub fn move_row(&mut self, delta: isize) {
        let count = self.rows().len();
        if count == 0 { self.selected_row = 0; return; }
        self.selected_row = (self.selected_row as isize + delta).clamp(0, count as isize - 1) as usize;
    }

    pub fn rows(&self) -> Vec<(String, String)> {
        use plx_os::storage::deck_vault::*;
        match self.selected_root {
            SettingsRoot::Playback => vec![
                ("Skip interval".into(), format!("{} seconds", self.settings.playback.skip_interval_sec.seconds())),
                ("Instant D-pad seeking".into(), on_off(self.settings.playback.instant_dpad_seeking)),
                ("HDR overlay brightness".into(), match self.settings.playback.hdr_overlay_brightness { HdrOverlayBrightness::Fifty => "50%".into(), HdrOverlayBrightness::SeventyFive => "75%".into(), HdrOverlayBrightness::OneHundred => "100%".into() }),
                ("Show remaining time".into(), on_off(self.settings.playback.hud_show_remaining)),
                ("HUD auto-hide".into(), match self.settings.playback.hud_auto_hide_sec { HudAutoHide::Three => "3 seconds".into(), HudAutoHide::Five => "5 seconds".into(), HudAutoHide::Eight => "8 seconds".into() }),
                ("Compact HUD".into(), on_off(self.settings.playback.hud_compact)),
                ("Continuously play next".into(), on_off(self.settings.playback.continuously_play_next)),
            ],
            SettingsRoot::AudioSubtitles => vec![
                ("Preferred audio language".into(), self.settings.audio_subtitles.preferred_audio_lang.clone()),
                ("Subtitles".into(), format!("{:?}", self.settings.audio_subtitles.subtitles_mode)),
                ("Subtitle language".into(), self.settings.audio_subtitles.subtitle_lang.clone()),
                ("Forced subtitles".into(), format!("{:?}", self.settings.audio_subtitles.forced_subtitles)),
                ("Subtitle size".into(), format!("{:?}", self.settings.audio_subtitles.subtitle_size)),
                ("Subtitle position".into(), format!("{:?}", self.settings.audio_subtitles.subtitle_position)),
                ("Subtitle color".into(), format!("{:?}", self.settings.audio_subtitles.subtitle_color)),
            ],
            SettingsRoot::Credentials => self.sources.iter().map(|source| {
                let credential = if source.kind == SourceKind::Smb && !source.guest {
                    if source.has_saved_credential { "Credential sealed" } else { "Credential not saved" }
                } else { "No credential required" };
                (source.label.clone(), credential.to_string())
            }).collect(),
            SettingsRoot::Recovery => vec![
                ("Auto-recover video errors".into(), on_off(self.settings.recovery.auto_recover_video_errors)),
                ("Prefer remux containers".into(), on_off(self.settings.recovery.prefer_remux_containers)),
                ("Sandbox video-plane rebind".into(), on_off(self.settings.recovery.sandbox_plane_rebind)),
            ],
        }
    }

    pub fn save(&mut self, vault: &mut DeckVault) -> Result<(), String> {
        match vault.save_settings(self.settings.clone()) {
            Ok(()) => { self.status = Some("Settings saved".into()); Ok(()) }
            Err(error) => { self.status = Some(error.clone()); Err(error) }
        }
    }

    pub fn clear_source_credential(&mut self, vault: &mut DeckVault, source_id: &str) -> Result<(), String> {
        vault.clear_credential(source_id)?;
        if let Some(source) = self.sources.iter_mut().find(|source| source.id == source_id) {
            source.has_saved_credential = false;
        }
        self.status = Some("Saved credential cleared".into());
        Ok(())
    }

    pub fn remove_source(&mut self, vault: &mut DeckVault, source_id: &str) -> Result<(), String> {
        vault.remove_source(source_id)?;
        self.sources.retain(|source| source.id != source_id);
        self.selected_row = self.selected_row.min(self.rows().len().saturating_sub(1));
        self.status = Some("Source removed".into());
        Ok(())
    }

    pub fn draw(&self, painter: Painter, bounds: Rect) {
        painter.rrect(bounds, 0.0, 0.0, theme::DECKFILES_SETTINGS);
        let left_w = 390.0;
        TextView::new("Settings", theme::size::HEADLINE, theme::TEXT_PRIMARY)
            .draw(painter, Rect::new(bounds.x + 36.0, bounds.y + 28.0, left_w - 60.0, 52.0));
        for (index, root) in SettingsRoot::ALL.iter().enumerate() {
            let y = bounds.y + 112.0 + index as f32 * 76.0;
            let selected = *root == self.selected_root;
            if selected { painter.rrect(Rect::new(bounds.x + 20.0, y, left_w - 40.0, 62.0), 10.0, 10.0, theme::CONTROL_IDLE_FILL); }
            TextView::new(root.title(), theme::size::BODY, if selected { theme::ACCENT_INK } else { theme::TEXT_SECONDARY })
                .draw(painter, Rect::new(bounds.x + 40.0, y + 10.0, left_w - 80.0, 42.0));
        }
        let right_x = bounds.x + left_w + 24.0;
        let right_w = (bounds.w - left_w - 60.0).max(0.0);
        TextView::new(self.selected_root.title(), theme::size::TITLE, theme::TEXT_PRIMARY)
            .draw(painter, Rect::new(right_x, bounds.y + 34.0, right_w, 52.0));
        for (index, (label, value)) in self.rows().iter().take(10).enumerate() {
            let y = bounds.y + 116.0 + index as f32 * 64.0;
            if index == self.selected_row { painter.rrect(Rect::new(right_x - 12.0, y, right_w, 56.0), 8.0, 8.0, theme::CONTROL_IDLE_FILL); }
            TextView::new(label, theme::size::CAPTION, if index == self.selected_row { theme::ACCENT_INK } else { theme::TEXT_SECONDARY })
                .draw(painter, Rect::new(right_x + 8.0, y + 8.0, right_w * 0.58, 40.0));
            TextView::new(value, theme::size::CAPTION, if index == self.selected_row { theme::ACCENT_INK } else { theme::TEXT_TERTIARY })
                .draw(painter, Rect::new(right_x + right_w * 0.60, y + 8.0, right_w * 0.36, 40.0));
        }
        if let Some(status) = &self.status {
            TextView::new(status, theme::size::CAPTION, theme::TEXT_SECONDARY)
                .draw(painter, Rect::new(right_x, bounds.y + bounds.h - 56.0, right_w, 36.0));
        }
    }
}

fn on_off(value: bool) -> String { if value { "On".into() } else { "Off".into() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_expose_exactly_four_roots() {
        assert_eq!(SettingsRoot::ALL.len(), 4);
        assert_eq!(SettingsRoot::ALL.map(SettingsRoot::title),
            ["Playback", "Audio & Subtitles", "Vault & Credentials", "Recovery"]);
    }
    #[test]
    fn default_settings_are_the_vault_defaults() {
        let screen = DeckSettingsScreen::default();
        assert!(screen.settings.playback.instant_dpad_seeking);
        assert!(screen.settings.recovery.auto_recover_video_errors);
    }
}
