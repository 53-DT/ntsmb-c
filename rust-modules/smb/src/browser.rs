//! Pure SMB file-browser presentation helpers.
//!
//! This module keeps the normal browser intentionally plain: folders first, then video files,
//! alphabetically, with no server metadata or artwork dependency. The UI can consume these helpers
//! without putting path policy or extension filtering into a render frame.

use crate::Entry;

/// A visible item in the SMB browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEntry {
    pub entry: Entry,
    /// True for a video file recognized by the extension allow-list.
    pub is_video: bool,
}

/// Video types supported by the file browser's MVP filter.
/// Playback support still depends on the TV decoder and the existing player.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "avi", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts", "mxf", "ts", "webm", "wmv",
];

/// Filter out non-video files and sort folders first, then visible files by case-insensitive name.
/// Folder rows are always retained regardless of their extension.
pub fn visible_entries(entries: impl IntoIterator<Item = Entry>) -> Vec<BrowserEntry> {
    let mut visible: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| {
            if entry.is_directory {
                Some(BrowserEntry { entry, is_video: false })
            } else if is_video_name(&entry.name) {
                Some(BrowserEntry { entry, is_video: true })
            } else {
                None
            }
        })
        .collect();

    visible.sort_by(|a, b| {
        b.entry.is_directory
            .cmp(&a.entry.is_directory)
            .then_with(|| a.entry.name.to_lowercase().cmp(&b.entry.name.to_lowercase()))
            .then_with(|| a.entry.name.cmp(&b.entry.name))
    });
    visible
}

/// Whether a filename's extension is one of the browser's video types.
pub fn is_video_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, extension)| VIDEO_EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)))
        .unwrap_or(false)
}

/// Join a current share-relative directory and one directory/file name.
/// Rejects names that could escape or introduce another path segment.
pub fn child_path(current: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("Invalid SMB entry name".into());
    }
    let current = current.trim_matches('/');
    if current.is_empty() {
        Ok(name.to_owned())
    } else {
        Ok(format!("{current}/{name}"))
    }
}

/// Split a share-relative path into breadcrumb labels. Root is represented by an empty vector.
pub fn breadcrumbs(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .map(str::to_owned)
        .collect()
}

/// Parent of a share-relative directory. Root has no parent.
pub fn parent_path(path: &str) -> Option<String> {
    let normalized = path.trim_matches('/');
    if normalized.is_empty() {
        return None;
    }
    normalized.rsplit_once('/').map(|(parent, _)| parent.to_owned()).or(Some(String::new()))
}


/// The file browser's logical presentation state. This is deliberately independent of network IO:
/// the app worker supplies a directory listing, while the screen owns path, cursor and navigation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BrowserModel {
    /// Share-relative directory; empty means the share root.
    path: String,
    /// Visible rows after filtering and sorting.
    entries: Vec<BrowserEntry>,
    /// Focused row index, clamped whenever a listing changes.
    selected: usize,
    /// Last selected child directory, used when returning from a nested folder.
    parent_selection: Option<(String, usize)>,
    /// Listing error shown in the browser read-out.
    error: Option<String>,
    loading: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserAction {
    OpenDirectory { path: String, restore_selection: usize },
    PlayFile { path: String },
    None,
}

impl BrowserModel {
    pub fn path(&self) -> &str { &self.path }
    pub fn entries(&self) -> &[BrowserEntry] { &self.entries }
    pub fn selected(&self) -> usize { self.selected }
    pub fn error(&self) -> Option<&str> { self.error.as_deref() }
    pub fn is_loading(&self) -> bool { self.loading }
    pub fn crumbs(&self) -> Vec<String> { breadcrumbs(&self.path) }

    /// Begin a directory request. The current row is retained so BACK can restore it.
    pub fn begin_open(&mut self, path: String, restore_selection: usize) {
        self.loading = true;
        self.error = None;
        self.parent_selection = Some((path.clone(), restore_selection));
        self.path = path;
        self.entries.clear();
        self.selected = 0;
    }

    /// Publish a successful worker result and seat focus at the requested row.
    pub fn publish(&mut self, rows: impl IntoIterator<Item = Entry>, selection: usize) {
        self.entries = visible_entries(rows);
        self.selected = selection.min(self.entries.len().saturating_sub(1));
        self.loading = false;
        self.error = None;
    }

    /// Publish a failed directory request without losing the current path.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.loading = false;
        self.error = Some(message.into());
        self.entries.clear();
        self.selected = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() { self.selected = 0; return; }
        let last = self.entries.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Resolve OK on the focused row. The caller performs network navigation or native playback.
    pub fn activate(&self) -> BrowserAction {
        let Some(row) = self.entries.get(self.selected) else { return BrowserAction::None; };
        match child_path(&self.path, &row.entry.name) {
            Ok(path) if row.entry.is_directory => BrowserAction::OpenDirectory {
                path,
                restore_selection: self.selected,
            },
            Ok(path) if row.is_video => BrowserAction::PlayFile { path },
            _ => BrowserAction::None,
        }
    }

    /// Resolve BACK. Returns the parent path and the child row to restore focus to.
    pub fn back(&self) -> Option<(String, usize)> {
        let parent = parent_path(&self.path)?;
        let selection = self.parent_selection
            .as_ref()
            .filter(|(opened, _)| opened == &self.path)
            .map(|(_, selection)| *selection)
            .unwrap_or(0);
        Some((parent, selection))
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, is_directory: bool) -> Entry {
        Entry { name: name.into(), size: 0, is_directory }
    }

    #[test]
    fn folders_first_then_video_files_alphabetically() {
        let actual = visible_entries([
            row("zeta.mp4", false),
            row("Docs", true),
            row("alpha.mkv", false),
            row("music.mp3", false),
            row("Movies", true),
            row("hidden.txt", false),
        ]);
        assert_eq!(actual.iter().map(|item| item.entry.name.as_str()).collect::<Vec<_>>(),
            ["Docs", "Movies", "alpha.mkv", "zeta.mp4"]);
    }

    #[test]
    fn extension_filter_is_case_insensitive_and_handles_no_extension() {
        assert!(is_video_name("episode.MKV"));
        assert!(is_video_name("movie.m2ts"));
        assert!(!is_video_name("cover.jpg"));
        assert!(!is_video_name("README"));
    }

    #[test]
    fn child_path_rejects_traversal_and_path_separators() {
        assert_eq!(child_path("Movies", "Film.mkv").unwrap(), "Movies/Film.mkv");
        assert_eq!(child_path("", "Films").unwrap(), "Films");
        assert!(child_path("Movies", "..").is_err());
        assert!(child_path("Movies", "Other/Film.mkv").is_err());
        assert!(child_path("Movies", "Other\\Film.mkv").is_err());
    }

    #[test]
    fn breadcrumbs_and_parent_path_are_share_relative() {
        assert_eq!(breadcrumbs("Movies/Drama/"), ["Movies", "Drama"]);
        assert_eq!(breadcrumbs(""), Vec::<String>::new());
        assert_eq!(parent_path("Movies/Drama"), Some("Movies".into()));
        assert_eq!(parent_path("Movies"), Some(String::new()));
        assert_eq!(parent_path(""), None);
    }

    #[test]
    fn browser_model_keeps_cursor_and_returns_folder_and_file_actions() {
        let mut model = BrowserModel::default();
        model.publish([
            row("Film.mkv", false), row("A folder", true), row("poster.jpg", false),
        ], 1);
        assert_eq!(model.selected(), 1);
        assert_eq!(model.activate(), BrowserAction::OpenDirectory {
            path: "A folder".into(), restore_selection: 1,
        });
        model.begin_open("A folder".into(), 1);
        model.publish([row("Episode.mp4", false)], 0);
        assert_eq!(model.activate(), BrowserAction::PlayFile { path: "A folder/Episode.mp4".into() });
        assert_eq!(model.back(), Some((String::new(), 1)));
    }

    #[test]
    fn browser_model_clamps_selection_and_exposes_errors() {
        let mut model = BrowserModel::default();
        model.publish([row("a.mkv", false), row("b.mkv", false)], 99);
        assert_eq!(model.selected(), 1);
        model.fail("Share disconnected");
        assert_eq!(model.error(), Some("Share disconnected"));
        assert!(!model.is_loading());
        assert!(model.entries().is_empty());
    }

}
