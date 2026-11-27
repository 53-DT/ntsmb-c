//! DeckFiles' provider picker and source-connection form.
//!
//! Passwords exist only in this live form and the provider worker's payload. Persistence is
//! delegated to DeckVault, which seals credentials through plx_os::tv::secure.

use std::sync::mpsc::{self, Receiver};
use plx_os::storage::deck_vault::{DeckVault, SourceKind, SourceRecord};
use plx_smb::provider::{ConnectPayload, SourceRegistry};
use plx_ui::{Painter, Rect};
use plx_ui::text_view::TextView;
use plx_ui::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectState { #[default] Editing, Connecting, Connected }

pub struct AddSourceModal {
    pub kind: SourceKind,
    pub label: String,
    pub host: String,
    pub port: String,
    pub share: String,
    pub username: String,
    pub password: String,
    pub guest: bool,
    pub local_root: String,
    pub state: ConnectState,
    pub error: Option<String>,
}

impl Default for AddSourceModal {
    fn default() -> Self { Self::new() }
}

impl AddSourceModal {
    pub fn new() -> Self {
        Self { kind: SourceKind::Smb, label: String::new(), host: String::new(), port: "445".into(),
            share: String::new(), username: String::new(), password: String::new(), guest: false,
            local_root: String::new(), state: ConnectState::Editing, error: None }
    }

    pub fn providers() -> [(SourceKind, &'static str, bool); 3] {
        [(SourceKind::Smb, "SMB", true), (SourceKind::Local, "Local Media", true),
         (SourceKind::GoogleDriveSlot, "Google Drive", false)]
    }

    pub fn select_provider(&mut self, kind: SourceKind) -> Result<(), String> {
        if !Self::providers().iter().any(|(candidate, _, available)| *candidate == kind && *available) {
            return Err("This provider is not available yet".into());
        }
        self.kind = kind;
        self.error = None;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            SourceKind::Smb => {
                if self.host.trim().is_empty() { return Err("Enter the SMB server address".into()); }
                let port = self.port.trim().parse::<u16>().map_err(|_| "Enter a valid SMB port".to_string())?;
                if port == 0 { return Err("Enter a valid SMB port".into()); }
                if !self.guest && self.username.trim().is_empty() { return Err("Enter a username or enable Guest".into()); }
                if !self.guest && self.password.is_empty() { return Err("Enter the SMB password".into()); }
            }
            SourceKind::Local => {
                if self.local_root.trim().is_empty() { return Err("Choose a local media folder".into()); }
            }
            SourceKind::GoogleDriveSlot | SourceKind::WebDavSlot => return Err("This provider is not available yet".into()),
        }
        Ok(())
    }

    pub fn payload(&self) -> Result<ConnectPayload, String> {
        self.validate()?;
        Ok(ConnectPayload {
            kind: self.kind,
            label: self.label.trim().to_owned(),
            host: self.host.trim().to_owned(),
            port: if self.kind == SourceKind::Smb { self.port.trim().parse().unwrap_or(445) } else { 0 },
            share: self.share.trim().to_owned(),
            username: self.username.trim().to_owned(),
            guest: self.guest,
            secret: (self.kind == SourceKind::Smb && !self.guest).then(|| self.password.clone()),
            root_path: self.local_root.trim().to_owned(),
        })
    }

    /// Runs provider validation/connect work off the UI thread. The vault is moved into the worker
    /// and returned with its result so only one owner can mutate its persisted state.
    pub fn start_connect(&mut self, vault: DeckVault) -> Result<Receiver<(DeckVault, Result<SourceRecord, String>)>, String> {
        let payload = match self.payload() {
            Ok(payload) => payload,
            Err(error) => { self.error = Some(error.clone()); return Err(error); }
        };
        let kind = self.kind;
        self.state = ConnectState::Connecting;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("deckfiles-connect".into()).spawn(move || {
            let registry = SourceRegistry::new();
            let result = registry.get(kind)
                .ok_or_else(|| "This provider is not available yet".to_string())
                .and_then(|provider| provider.connect(&payload, &mut vault));
            // The worker returns the vault in both success and failure cases.
            // connect() persists only after a successful provider validation.
            let _ = tx.send((vault, result));
        }).map_err(|_| { self.state = ConnectState::Editing; "Could not start the connection worker".to_string() })?;
        Ok(rx)
    }

    pub fn finish_connect(&mut self, result: &Result<SourceRecord, String>) {
        match result {
            Ok(_) => { self.state = ConnectState::Connected; self.password.clear(); self.error = None; }
            Err(error) => { self.state = ConnectState::Editing; self.error = Some(error.clone()); }
        }
    }

    pub fn cancel(&mut self) {
        self.password.clear();
        self.state = ConnectState::Editing;
        self.error = None;
    }

    pub fn draw(&self, painter: Painter, bounds: Rect) {
        painter.rrect(bounds, 18.0, 18.0, theme::DECKFILES_CANVAS);
        TextView::new("Add source", theme::size::HEADLINE, theme::TEXT_PRIMARY)
            .draw(painter, Rect::new(bounds.x + 42.0, bounds.y + 28.0, bounds.w - 84.0, 52.0));
        TextView::new("Credentials are sealed on this device", theme::size::CAPTION, theme::TEXT_SECONDARY)
            .draw(painter, Rect::new(bounds.x + 42.0, bounds.y + 84.0, bounds.w - 84.0, 38.0));
        for (index, (_, title, available)) in Self::providers().iter().enumerate() {
            let x = bounds.x + 42.0 + index as f32 * 230.0;
            let active = Self::providers()[index].0 == self.kind;
            painter.rrect(Rect::new(x, bounds.y + 146.0, 210.0, 72.0), 10.0, 10.0,
                if active { theme::CONTROL_IDLE_FILL } else { theme::SURFACE_PANEL });
            TextView::new(title, theme::size::BODY, if active { theme::ACCENT_INK } else { theme::TEXT_SECONDARY })
                .draw(painter, Rect::new(x + 16.0, bounds.y + 164.0, 178.0, 36.0));
            if !available {
                TextView::new("Coming soon", theme::size::MICRO, theme::TEXT_TERTIARY)
                    .draw(painter, Rect::new(x + 16.0, bounds.y + 218.0, 178.0, 30.0));
            }
        }
        let fields: Vec<String> = if self.kind == SourceKind::Smb {
            vec![format!("Name: {}", self.label), format!("Server: {}", self.host), format!("Port: {}", self.port),
                 format!("Share: {}", self.share), format!("Username: {}", self.username),
                 format!("Password: {}", "•".repeat(self.password.chars().count())),
                 format!("Guest: {}", if self.guest { "On" } else { "Off" })]
        } else { vec![format!("Name: {}", self.label), format!("Folder: {}", self.local_root)] };
        for (index, field) in fields.iter().enumerate() {
            TextView::new(field, theme::size::CAPTION, theme::TEXT_SECONDARY)
                .draw(painter, Rect::new(bounds.x + 42.0, bounds.y + 270.0 + index as f32 * 42.0, bounds.w - 84.0, 36.0));
        }
        if let Some(error) = &self.error {
            painter.rrect(Rect::new(bounds.x + 36.0, bounds.y + bounds.h - 132.0, bounds.w - 72.0, 46.0), 8.0, 8.0, theme::CONTROL_DANGER_IDLE_FILL);
            TextView::new(error, theme::size::CAPTION, theme::DANGER)
                .draw(painter, Rect::new(bounds.x + 52.0, bounds.y + bounds.h - 126.0, bounds.w - 104.0, 34.0));
        }
        let footer = match self.state { ConnectState::Connecting => "Connecting…", ConnectState::Connected => "Connected", ConnectState::Editing => "Back     Cancel     Connect" };
        TextView::new(footer, theme::size::CAPTION, theme::TEXT_PRIMARY)
            .draw(painter, Rect::new(bounds.x + 42.0, bounds.y + bounds.h - 66.0, bounds.w - 84.0, 42.0));
    }
}
