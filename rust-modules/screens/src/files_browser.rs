//! DeckFiles' provider-neutral Files surface model and renderer.
//!
//! This module is intentionally independent of the legacy Plex screens. The app mounts it in
//! Phase 4; all browsing data comes from the neutral SMB provider contract and OS vault.

use std::sync::mpsc::{self, Receiver};
use plx_os::storage::deck_vault::{DeckVault, SourceRecord, SourceKind};
use plx_smb::provider::{EntryKind, MediaEntry, SourceRegistry};
use plx_ui::{Painter, Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowserMode { #[default] Grid, List }

pub struct FilesBrowserScreen {
    pub sources: Vec<SourceRecord>,
    pub active_source: Option<String>,
    pub current_path: String,
    pub entries: Vec<MediaEntry>,
    pub selected: usize,
    pub mode: BrowserMode,
    pub search: String,
    pub loading: bool,
    pub error: Option<String>,
    pub select_mode: bool,
    history: Vec<String>,
}

impl Default for FilesBrowserScreen {
    fn default() -> Self { Self::new() }
}

impl FilesBrowserScreen {
    pub fn new() -> Self {
        Self { sources: Vec::new(), active_source: None, current_path: String::new(),
            entries: Vec::new(), selected: 0, mode: BrowserMode::Grid, search: String::new(),
            loading: false, error: None, select_mode: false, history: Vec::new() }
    }

    pub fn set_sources(&mut self, sources: &[SourceRecord]) {
        self.sources.clear();
        self.sources.extend_from_slice(sources);
        if self.active_source.as_ref().is_some_and(|id| !self.sources.iter().any(|s| &s.id == id)) {
            self.active_source = None;
            self.current_path.clear();
            self.entries.clear();
            self.history.clear();
            self.selected = 0;
        }
    }

    pub fn choose_source(&mut self, id: &str) -> bool {
        if !self.sources.iter().any(|source| source.id == id) { return false; }
        self.active_source = Some(id.to_owned());
        self.current_path.clear();
        self.history.clear();
        self.entries.clear();
        self.selected = 0;
        self.error = None;
        true
    }

    pub fn start_browse(&mut self, vault: DeckVault) -> Result<Receiver<(DeckVault, Result<Vec<MediaEntry>, String>)>, String> {
        let id = self.active_source.clone().ok_or_else(|| "Choose a source first".to_string())?;
        let source = self.sources.iter().find(|source| source.id == id).cloned()
            .ok_or_else(|| "The selected source is no longer saved".to_string())?;
        let path = self.current_path.clone();
        self.loading = true;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("deckfiles-browse".into()).spawn(move || {
            let registry = SourceRegistry::new();
            let result = registry.get(source.kind)
                .ok_or_else(|| "This source provider is unavailable".to_string())
                .and_then(|provider| provider.list_dir(&source, &path, &vault));
            let _ = tx.send((vault, result));
        }).map_err(|_| { self.loading = false; "Could not start the source browser".to_string() })?;
        Ok(rx)
    }

    pub fn finish_browse(&mut self, result: Result<Vec<MediaEntry>, String>) {
        self.loading = false;
        match result {
            Ok(mut entries) => {
                entries.sort_by(|a, b| {
                    b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name))
                });
                self.entries = entries;
                self.selected = self.selected.min(self.visible_entries().len().saturating_sub(1));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn visible_entries(&self) -> Vec<&MediaEntry> {
        let query = self.search.trim().to_lowercase();
        self.entries.iter().filter(|entry| query.is_empty() || entry.name.to_lowercase().contains(&query)).collect()
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.visible_entries().len();
        if len == 0 { self.selected = 0; return; }
        self.selected = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
    }

    pub fn selected_entry(&self) -> Option<&MediaEntry> {
        self.visible_entries().get(self.selected).copied()
    }

    pub fn enter_selected(&mut self) -> Option<String> {
        let entry = self.selected_entry()?.clone();
        if !entry.is_dir { return None; }
        self.history.push(self.current_path.clone());
        self.current_path = entry.path;
        self.selected = 0;
        self.entries.clear();
        Some(self.current_path.clone())
    }

    pub fn go_back(&mut self) -> Option<String> {
        self.current_path = self.history.pop()?;
        self.selected = 0;
        self.entries.clear();
        Some(self.current_path.clone())
    }

    pub fn breadcrumb(&self) -> Vec<&str> {
        let mut parts = vec!["Files"];
        parts.extend(self.current_path.split('/').filter(|part| !part.is_empty()));
        parts
    }

    pub fn playlist_paths(&self, selected_file: &str) -> Vec<String> {
        self.entries.iter().filter(|entry| entry.kind == EntryKind::Video)
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .filter(|path| !path.is_empty())
            .fold(Vec::new(), |mut paths, path| { paths.push(path); paths })
            .tap_selected(selected_file)
    }

    pub fn remove_source(&mut self, vault: &mut DeckVault, id: &str) -> Result<(), String> {
        vault.remove_source(id)?;
        self.sources.retain(|source| source.id != id);
        if self.active_source.as_deref() == Some(id) {
            self.active_source = None;
            self.current_path.clear();
            self.entries.clear();
            self.history.clear();
            self.selected = 0;
        }
        Ok(())
    }

    pub fn draw(&self, painter: Painter, bounds: Rect) {
        let sidebar = Rect::new(bounds.x, bounds.y, 300.0, bounds.h);
        let content = Rect::new(bounds.x + 300.0, bounds.y, (bounds.w - 300.0).max(0.0), bounds.h);
        painter.rrect(sidebar, 0.0, 0.0, theme::DECKFILES_SIDEBAR);
        painter.rrect(content, 0.0, 0.0, theme::DECKFILES_CANVAS);
        TextView::new("Files", theme::size::HEADLINE, theme::TEXT_PRIMARY).draw(painter, Rect::new(sidebar.x + 28.0, sidebar.y + 30.0, 240.0, 48.0));
        TextView::new("Settings", theme::size::BODY, theme::TEXT_SECONDARY).draw(painter, Rect::new(sidebar.x + 28.0, sidebar.y + 92.0, 240.0, 42.0));
        TextView::new("SOURCES", theme::size::MICRO, theme::TEXT_TERTIARY).draw(painter, Rect::new(sidebar.x + 28.0, sidebar.y + 166.0, 240.0, 32.0));
        TextView::new("+ Add source", theme::size::CAPTION, theme::TEXT_PRIMARY).draw(painter, Rect::new(sidebar.x + 28.0, sidebar.y + 206.0, 240.0, 38.0));
        for (index, source) in self.sources.iter().enumerate() {
            let y = sidebar.y + 260.0 + index as f32 * 52.0;
            let active = self.active_source.as_deref() == Some(source.id.as_str());
            if active { painter.rrect(Rect::new(sidebar.x + 12.0, y - 4.0, 276.0, 46.0), 8.0, 8.0, theme::CONTROL_IDLE_FILL); }
            TextView::new(&source.label, theme::size::CAPTION, if active { theme::ACCENT_INK } else { theme::TEXT_SECONDARY })
                .draw(painter, Rect::new(sidebar.x + 28.0, y, 244.0, 36.0));
        }
        let crumb = self.breadcrumb().join("  ›  ");
        TextView::new(&crumb, theme::size::BODY, theme::TEXT_PRIMARY).draw(painter, Rect::new(content.x + 36.0, content.y + 26.0, content.w - 72.0, 48.0));
        TextView::new("Select     Grid / List     Search", theme::size::CAPTION, theme::TEXT_SECONDARY)
            .draw(painter, Rect::new(content.x + 36.0, content.y + 88.0, content.w - 72.0, 38.0));
        if self.loading {
            TextView::new("Loading…", theme::size::BODY, theme::TEXT_SECONDARY).draw(painter, Rect::new(content.x + 36.0, content.y + 170.0, content.w - 72.0, 44.0));
        } else if let Some(error) = &self.error {
            TextView::new(error, theme::size::CAPTION, theme::DANGER).draw(painter, Rect::new(content.x + 36.0, content.y + 170.0, content.w - 72.0, 64.0));
        } else {
            let rows = self.visible_entries();
            let row_h = if self.mode == BrowserMode::List { 58.0 } else { 76.0 };
            for (index, entry) in rows.iter().take(10).enumerate() {
                let y = content.y + 154.0 + index as f32 * row_h;
                let selected = index == self.selected;
                if selected { painter.rrect(Rect::new(content.x + 24.0, y, content.w - 48.0, row_h - 6.0), 8.0, 8.0, theme::CONTROL_IDLE_FILL); }
                let prefix = if entry.is_dir { "▸  " } else if entry.kind == EntryKind::Video { "▶  " } else { "   " };
                let label = format!("{prefix}{}", entry.name);
                TextView::new(&label, theme::size::BODY, if selected { theme::ACCENT_INK } else { theme::TEXT_SECONDARY })
                    .draw(painter, Rect::new(content.x + 42.0, y + 5.0, content.w - 84.0, row_h - 12.0));
            }
        }
    }
}

trait SelectedPath { fn tap_selected(self, selected: &str) -> Vec<String>; }
impl SelectedPath for Vec<String> {
    fn tap_selected(self, selected: &str) -> Vec<String> {
        if self.iter().any(|path| path == selected) { self } else { Vec::new() }
    }
}

fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let mut a = left.chars().peekable();
    let mut b = right.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut an = String::new(); while a.peek().is_some_and(|c| c.is_ascii_digit()) { an.push(a.next().unwrap()); }
                let mut bn = String::new(); while b.peek().is_some_and(|c| c.is_ascii_digit()) { bn.push(b.next().unwrap()); }
                let ord = an.trim_start_matches('0').len().cmp(&bn.trim_start_matches('0').len())
                    .then_with(|| an.trim_start_matches('0').cmp(bn.trim_start_matches('0')));
                if ord != std::cmp::Ordering::Equal { return ord; }
            }
            _ => {
                let x = a.next().unwrap().to_ascii_lowercase();
                let y = b.next().unwrap().to_ascii_lowercase();
                let ord = x.cmp(&y);
                if ord != std::cmp::Ordering::Equal { return ord; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn breadcrumbs_are_built_from_the_current_relative_path() {
        let mut screen = FilesBrowserScreen::new();
        screen.current_path = "Films/2026".into();
        assert_eq!(screen.breadcrumb(), vec!["Files", "Films", "2026"]);
    }
    #[test]
    fn natural_sort_compares_numeric_runs() {
        assert_eq!(natural_cmp("Episode 2.mkv", "Episode 10.mkv"), std::cmp::Ordering::Less);
    }
    #[test]
    fn playlist_context_is_only_the_selected_video() {
        let mut screen = FilesBrowserScreen::new();
        screen.entries = vec![
            MediaEntry { name: "a.mkv".into(), path: "a.mkv".into(), is_dir: false, kind: EntryKind::Video, size_bytes: 1 },
            MediaEntry { name: "b.srt".into(), path: "b.srt".into(), is_dir: false, kind: EntryKind::Subtitle, size_bytes: 1 },
        ];
        assert_eq!(screen.playlist_paths("a.mkv"), vec!["a.mkv"]);
        assert!(screen.playlist_paths("b.srt").is_empty());
    }
}
