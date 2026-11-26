//! The one object the SMB screens talk to.
//!
//! It owns the [`SmbWorker`] (all network I/O stays on that thread), the folder model, and the
//! navigation memory, and it turns the worker's events into a small, plain state the Connect and
//! Browse screens can draw. It has no UI, no focus and no drawing: the screens read it, and the
//! application pumps it once per frame. That keeps the sans-IO rule of the screens crate intact
//! (a screen starts no worker; it asks the application, which owns this).
//!
//! What it adds beyond `SmbWorker` + `BrowserModel`:
//! * **A selection stack**, so going Back through several folders puts the cursor on the folder you
//!   came out of at EVERY level (`BrowserModel` alone only remembers one level).
//! * **Share picking**: a server with one share opens it at once; a saved server opens the share it
//!   was saved with; otherwise the person picks from the real share list.
//! * **Resume**: re-open the folder a saved server was last in, falling back to the share root if
//!   that folder is gone.
//! * **A play request** that carries everything a SECOND, independent connection needs. Playback
//!   hands its session to the range bridge for the life of the film (see `playback.rs`), so it must
//!   not consume the browsing session: Back from the player lands on a live listing.
//!
//! Nothing here logs. Errors are short user-facing strings that never contain a credential.

use crate::browser::{breadcrumbs, parent_path, BrowserAction, BrowserModel};
use crate::connect_form::ConnectRequest;
use crate::worker::{Event, SmbWorker};
use crate::{Entry, ShareInfo};

/// Where the connection is, in the words a screen needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing started, or disconnected.
    Idle,
    /// Authenticating and reading the share list.
    Connecting,
    /// Authenticated; several shares to choose from.
    PickShare,
    /// A share was chosen and its root is being read.
    OpeningShare,
    /// A folder is on screen (it may be loading or may have failed: see `browser()`).
    Browsing,
    /// The connection itself failed. `message()` says why.
    Failed,
}

/// Everything a second connection needs to play one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayRequest {
    pub request: ConnectRequest,
    pub share: String,
    /// Relative to the share root, forward slashes.
    pub path: String,
}

/// What pressing OK on the focused row means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// Nothing to do (a folder was opened, or the row was not actionable).
    None,
    /// Start playback. See [`spawn_prepare`].
    Play(PlayRequest),
}

/// What pressing Back did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Back {
    /// Handled inside the browser (went up a folder, or back to the share list).
    Handled,
    /// Nothing left to go back through here: leave the browser (to the saved-servers / Connect screen).
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Awaiting {
    Nothing,
    Root,
    Folder { selection: usize },
}

pub struct BrowseController {
    worker: Option<SmbWorker>,
    request: Option<ConnectRequest>,
    phase: Phase,
    shares: Vec<ShareInfo>,
    share: Option<String>,
    browser: BrowserModel,
    /// One entry per open folder level: the row to put the cursor back on when coming out of it.
    stack: Vec<usize>,
    awaiting: Awaiting,
    auto_share: Option<String>,
    resume_path: Option<String>,
    resuming: bool,
    message: Option<String>,
    revision: u64,
}

impl Default for BrowseController {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowseController {
    pub fn new() -> Self {
        Self {
            worker: None,
            request: None,
            phase: Phase::Idle,
            shares: Vec::new(),
            share: None,
            browser: BrowserModel::default(),
            stack: Vec::new(),
            awaiting: Awaiting::Nothing,
            auto_share: None,
            resume_path: None,
            resuming: false,
            message: None,
            revision: 0,
        }
    }

    // ---- reading the state (every screen frame) ----

    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn shares(&self) -> &[ShareInfo] {
        &self.shares
    }
    pub fn share(&self) -> Option<&str> {
        self.share.as_deref()
    }
    pub fn browser(&self) -> &BrowserModel {
        &self.browser
    }
    /// A short, credential-free sentence for the screen to show, if there is one.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
    /// Moves on every change a screen could draw, so it can redraw only then.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// The folder on screen relative to the share root, no leading or trailing slash.
    pub fn folder(&self) -> &str {
        self.browser.path()
    }
    /// `host / share / Movies / Action`, for the path line above the list.
    pub fn location(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(request) = &self.request {
            parts.push(request.profile.host.clone());
        }
        if let Some(share) = &self.share {
            parts.push(share.clone());
        }
        parts.extend(breadcrumbs(self.browser.path()));
        parts.join(" / ")
    }
    /// The connection the person made, for "Save this server".
    pub fn request(&self) -> Option<&ConnectRequest> {
        self.request.as_ref()
    }

    // ---- starting ----

    /// Connect and read the share list. If the profile names a share it is opened as soon as the
    /// server lists it. Call [`resume_in`](Self::resume_in) first to also re-open a folder.
    pub fn connect(&mut self, request: ConnectRequest) -> Result<(), String> {
        if self.worker.is_none() {
            self.worker = Some(SmbWorker::start()?);
        }
        self.clear_view();
        self.auto_share = request.profile.share.clone();
        self.request = Some(request.clone());
        let worker = self.worker.as_ref().ok_or_else(|| "SMB worker is not available".to_string())?;
        worker.connect(request)?;
        self.phase = Phase::Connecting;
        self.revision += 1;
        Ok(())
    }

    /// After the next share opens, open this folder inside it (a saved server's last place).
    pub fn resume_in(&mut self, folder: &str) {
        let folder = folder.trim_matches('/');
        self.resume_path = if folder.is_empty() { None } else { Some(folder.to_owned()) };
    }

    pub fn select_share(&mut self, name: &str) {
        self.share = Some(name.to_owned());
        self.stack.clear();
        self.browser.reset();
        self.browser.begin_open(String::new(), 0);
        self.awaiting = Awaiting::Root;
        self.phase = Phase::OpeningShare;
        self.message = None;
        let name = name.to_owned();
        self.dispatch(move |w| w.select_share(name));
        self.revision += 1;
    }

    pub fn disconnect(&mut self) {
        self.dispatch(|w| w.disconnect());
        self.clear_view();
        self.request = None;
        self.phase = Phase::Idle;
        self.revision += 1;
    }

    // ---- the per-frame pump ----

    /// Drain the worker's events. Call once per frame on the frame thread; it never blocks.
    /// Answers whether anything a screen draws changed.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        loop {
            let next = match self.worker.as_ref() {
                Some(worker) => worker.try_event(),
                None => return changed,
            };
            match next {
                Ok(Some(event)) => changed |= self.apply_event(event),
                Ok(None) => return changed,
                Err(error) => {
                    self.worker = None;
                    self.fail(error);
                    return true;
                }
            }
        }
    }

    /// One worker event. Public so the state machine can be tested without a network.
    pub fn apply_event(&mut self, event: Event) -> bool {
        match event {
            Event::Shares(Ok(shares)) => self.shares_ready(shares),
            Event::Shares(Err(error)) => self.fail(error),
            Event::Directory(Ok(entries)) => self.directory_ready(entries),
            Event::Directory(Err(error)) => self.directory_failed(error),
            Event::Disconnected => {
                self.clear_view();
                self.phase = Phase::Idle;
            }
        }
        self.revision += 1;
        true
    }

    fn shares_ready(&mut self, shares: Vec<ShareInfo>) {
        self.shares = shares;
        self.message = None;
        let wanted = self.auto_share.take();
        let named = wanted.and_then(|wanted| {
            self.shares.iter().find(|s| s.name.eq_ignore_ascii_case(&wanted)).map(|s| s.name.clone())
        });
        let only = if self.shares.len() == 1 { Some(self.shares[0].name.clone()) } else { None };
        match named.or(only) {
            Some(name) => self.select_share(&name),
            None => {
                self.phase = Phase::PickShare;
                if self.shares.is_empty() {
                    self.message = Some("This server shares no folders with this account".into());
                }
            }
        }
    }

    fn directory_ready(&mut self, entries: Vec<Entry>) {
        self.resuming = false;
        let (selection, was_root) = match self.awaiting {
            Awaiting::Folder { selection } => (selection, false),
            Awaiting::Root => (0, true),
            Awaiting::Nothing => (0, false),
        };
        self.awaiting = Awaiting::Nothing;
        self.browser.publish(entries, selection);
        self.phase = Phase::Browsing;
        if was_root {
            if let Some(path) = self.resume_path.take() {
                // One zero per level: coming back out lands on the first row at each level.
                self.stack = vec![0; breadcrumbs(&path).len()];
                self.resuming = true;
                self.browser.begin_open(path.clone(), 0);
                self.awaiting = Awaiting::Folder { selection: 0 };
                self.dispatch(move |w| w.list_directory(path));
            }
        }
    }

    fn directory_failed(&mut self, error: String) {
        match self.awaiting {
            Awaiting::Root => {
                self.awaiting = Awaiting::Nothing;
                self.share = None;
                self.browser.reset();
                self.message = Some(error);
                self.phase = if self.shares.len() > 1 { Phase::PickShare } else { Phase::Failed };
            }
            Awaiting::Folder { .. } if self.resuming => {
                // The saved folder is gone. Show the share root instead of an error.
                self.resuming = false;
                self.stack.clear();
                self.awaiting = Awaiting::Root;
                self.browser.begin_open(String::new(), 0);
                self.dispatch(|w| w.list_directory(String::new()));
            }
            _ => {
                // Stay in Browsing: the person can still press Back out of the folder that failed.
                self.awaiting = Awaiting::Nothing;
                self.browser.fail(error);
            }
        }
    }

    // ---- remote keys ----

    pub fn move_selection(&mut self, delta: isize) {
        let before = self.browser.selected();
        self.browser.move_selection(delta);
        if self.browser.selected() != before {
            self.revision += 1;
        }
    }

    /// OK on the focused row: open a folder, or ask for playback.
    pub fn activate(&mut self) -> Activation {
        if self.phase != Phase::Browsing || self.browser.is_loading() {
            return Activation::None;
        }
        match self.browser.activate() {
            BrowserAction::OpenDirectory { path, restore_selection } => {
                self.stack.push(restore_selection);
                self.browser.begin_open(path.clone(), 0);
                self.awaiting = Awaiting::Folder { selection: 0 };
                self.dispatch(move |w| w.list_directory(path));
                self.revision += 1;
                Activation::None
            }
            BrowserAction::PlayFile { path } => match (self.request.clone(), self.share.clone()) {
                (Some(request), Some(share)) => Activation::Play(PlayRequest { request, share, path }),
                _ => Activation::None,
            },
            BrowserAction::None => Activation::None,
        }
    }

    /// BACK: up a folder, else back to the share list, else leave.
    pub fn go_back(&mut self) -> Back {
        if self.phase != Phase::Browsing {
            return Back::Leave;
        }
        let here = self.browser.path().to_owned();
        if let Some(parent) = parent_path(&here) {
            let selection = self.stack.pop().unwrap_or(0);
            self.browser.begin_open(parent.clone(), 0);
            self.awaiting = Awaiting::Folder { selection };
            self.dispatch(move |w| w.list_directory(parent));
            self.revision += 1;
            Back::Handled
        } else if self.shares.len() > 1 {
            self.share = None;
            self.stack.clear();
            self.browser.reset();
            self.phase = Phase::PickShare;
            self.revision += 1;
            Back::Handled
        } else {
            Back::Leave
        }
    }

    // ---- internals ----

    fn clear_view(&mut self) {
        self.shares.clear();
        self.share = None;
        self.browser.reset();
        self.stack.clear();
        self.awaiting = Awaiting::Nothing;
        self.auto_share = None;
        self.resume_path = None;
        self.resuming = false;
        self.message = None;
    }

    fn fail(&mut self, message: String) {
        self.awaiting = Awaiting::Nothing;
        self.message = Some(message);
        self.phase = Phase::Failed;
    }

    /// Send one command to the worker. A missing worker is a failure, except in unit tests, which
    /// drive the state machine with events and have no worker.
    fn dispatch(&mut self, send: impl FnOnce(&SmbWorker) -> Result<(), String>) {
        let result = match self.worker.as_ref() {
            Some(worker) => Some(send(worker)),
            None => None,
        };
        match result {
            Some(Err(error)) => self.fail(error),
            Some(Ok(())) => {}
            None => {
                if !cfg!(test) {
                    self.fail("SMB worker is not available".into());
                }
            }
        }
    }
}

/// Open the file on its OWN connection, probe it and bring up the loopback range bridge, off the
/// frame thread. The result arrives on the returned channel; the application then hands it to the
/// player exactly as `dev/scenarios.rs::smb_playback_arm` does for the smoke test.
#[cfg(feature = "playback")]
pub fn spawn_prepare(
    play: PlayRequest,
) -> Result<std::sync::mpsc::Receiver<Result<crate::playback::PreparedPlayback, String>>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("ntsmb-prepare".into())
        .spawn(move || {
            let result = (|| {
                let server = play.request.connect_server()?;
                let session = server.connect_share_blocking(&play.share)?;
                crate::playback::prepare_playback(session, &play.path)
            })();
            let _ = tx.send(result);
        })
        .map_err(|error| format!("Could not start the playback worker: {error}"))?;
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect_form::ConnectForm;

    fn request() -> ConnectRequest {
        let mut form = ConnectForm::new();
        form.host = "nas.local".into();
        form.guest = true;
        form.validate().unwrap()
    }

    fn dir(name: &str) -> Entry {
        Entry { name: name.into(), size: 0, is_directory: true }
    }
    fn file(name: &str) -> Entry {
        Entry { name: name.into(), size: 1, is_directory: false }
    }
    fn share(name: &str) -> ShareInfo {
        ShareInfo { name: name.into(), comment: String::new() }
    }

    fn browsing() -> BrowseController {
        let mut c = BrowseController::new();
        c.request = Some(request());
        c.shares = vec![share("Shared"), share("Backup")];
        c.select_share("Shared");
        c.apply_event(Event::Directory(Ok(vec![file("a.mkv"), dir("Shows"), dir("Movies")])));
        c
    }

    #[test]
    fn one_share_opens_at_once_and_several_ask() {
        let mut one = BrowseController::new();
        one.apply_event(Event::Shares(Ok(vec![share("Shared")])));
        assert_eq!((one.phase(), one.share()), (Phase::OpeningShare, Some("Shared")));

        let mut many = BrowseController::new();
        many.apply_event(Event::Shares(Ok(vec![share("Shared"), share("Backup")])));
        assert_eq!(many.phase(), Phase::PickShare);
    }

    #[test]
    fn a_saved_share_is_opened_without_asking_whatever_its_case() {
        let mut c = BrowseController::new();
        c.auto_share = Some("backup".into());
        c.apply_event(Event::Shares(Ok(vec![share("Shared"), share("Backup")])));
        assert_eq!(c.share(), Some("Backup"));
    }

    #[test]
    fn folders_come_first_and_back_restores_the_cursor_at_every_level() {
        let mut c = browsing();
        let names: Vec<_> = c.browser().entries().iter().map(|e| e.entry.name.clone()).collect();
        assert_eq!(names, ["Movies", "Shows", "a.mkv"], "folders first, then videos");

        c.move_selection(1); // Shows
        assert_eq!(c.activate(), Activation::None);
        assert_eq!(c.folder(), "Shows");
        c.apply_event(Event::Directory(Ok(vec![dir("Season 1"), dir("Season 2")])));
        c.move_selection(1); // Season 2
        c.activate();
        assert_eq!(c.folder(), "Shows/Season 2");
        c.apply_event(Event::Directory(Ok(vec![file("e1.mkv")])));

        assert_eq!(c.go_back(), Back::Handled);
        assert_eq!(c.folder(), "Shows");
        c.apply_event(Event::Directory(Ok(vec![dir("Season 1"), dir("Season 2")])));
        assert_eq!(c.browser().selected(), 1, "back on Season 2");

        assert_eq!(c.go_back(), Back::Handled);
        c.apply_event(Event::Directory(Ok(vec![file("a.mkv"), dir("Shows"), dir("Movies")])));
        assert_eq!(c.folder(), "");
        assert_eq!(c.browser().selected(), 1, "back on Shows");
    }

    #[test]
    fn a_video_asks_for_playback_on_its_own_connection() {
        let mut c = browsing();
        c.move_selection(2); // a.mkv
        match c.activate() {
            Activation::Play(play) => {
                assert_eq!((play.share.as_str(), play.path.as_str()), ("Shared", "a.mkv"));
            }
            other => panic!("expected playback, got {other:?}"),
        }
    }

    #[test]
    fn back_from_the_share_root_goes_to_the_share_list_or_leaves() {
        let mut many = browsing();
        assert_eq!(many.go_back(), Back::Handled);
        assert_eq!(many.phase(), Phase::PickShare);
        assert_eq!(many.go_back(), Back::Leave);

        let mut single = browsing();
        single.shares = vec![share("Shared")];
        assert_eq!(single.go_back(), Back::Leave);
    }

    #[test]
    fn a_share_that_will_not_open_returns_to_the_list_with_a_reason() {
        let mut c = BrowseController::new();
        c.apply_event(Event::Shares(Ok(vec![share("Shared"), share("Backup")])));
        c.select_share("Backup");
        c.apply_event(Event::Directory(Err("Could not connect to SMB share".into())));
        assert_eq!(c.phase(), Phase::PickShare);
        assert!(c.message().is_some());
        assert_eq!(c.share(), None);
    }

    #[test]
    fn a_failed_folder_keeps_back_working() {
        let mut c = browsing();
        c.activate(); // Movies
        c.apply_event(Event::Directory(Err("Could not list SMB directory".into())));
        assert_eq!(c.phase(), Phase::Browsing);
        assert!(c.browser().error().is_some());
        assert_eq!(c.go_back(), Back::Handled);
        assert_eq!(c.folder(), "");
    }

    #[test]
    fn resume_reopens_the_saved_folder_and_falls_back_to_the_root_when_it_is_gone() {
        let mut c = BrowseController::new();
        c.request = Some(request());
        c.shares = vec![share("Shared")];
        c.resume_in("Media/Shows");
        c.select_share("Shared");
        c.apply_event(Event::Directory(Ok(vec![dir("Media")])));
        assert_eq!(c.folder(), "Media/Shows");
        assert_eq!(c.stack.len(), 2);

        c.apply_event(Event::Directory(Err("not found".into())));
        assert_eq!(c.folder(), "", "fell back to the share root");
        assert!(c.stack.is_empty());
        c.apply_event(Event::Directory(Ok(vec![dir("Media")])));
        assert_eq!(c.phase(), Phase::Browsing);
        assert_eq!(c.browser().entries().len(), 1);
    }

    #[test]
    fn the_location_line_names_host_share_and_folders() {
        let mut c = browsing();
        c.activate();
        assert_eq!(c.location(), "nas.local / Shared / Movies");
    }
}