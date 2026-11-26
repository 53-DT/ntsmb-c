//! Dedicated SMB I/O worker.
//!
//! The TV render loop must never perform SMB network operations. This worker owns the live
//! authenticated server/share handles and serializes connection, share-selection and directory
//! requests on one background thread. The UI/app boundary sends commands and drains events.

use crate::{connect_form::ConnectRequest, Entry, Session, ShareInfo};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

enum Command {
    Connect(ConnectRequest),
    SelectShare(String),
    ListDirectory(String),
    Disconnect,
    Shutdown,
}

/// Results published by the worker. Errors are user-displayable and never include credentials.
#[derive(Debug)]
pub enum Event {
    Shares(Result<Vec<ShareInfo>, String>),
    Directory(Result<Vec<Entry>, String>),
    Disconnected,
}

/// A non-blocking command/event door to the dedicated SMB thread.
///
/// Keep this object in the application owner. Call its send methods from the UI/app boundary and
/// drain `try_event` during normal frame processing. All network I/O stays on the worker.
pub struct SmbWorker {
    commands: Sender<Command>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl SmbWorker {
    pub fn start() -> Result<Self, String> {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("ntsmb-smb-io".into())
            .spawn(move || worker_loop(command_rx, event_tx))
            .map_err(|error| format!("Could not start SMB worker: {error}"))?;
        Ok(Self { commands: command_tx, events: event_rx, thread: Some(thread) })
    }

    /// Authenticate and enumerate shares without blocking the caller.
    pub fn connect(&self, request: ConnectRequest) -> Result<(), String> {
        self.commands.send(Command::Connect(request))
            .map_err(|_| "SMB worker is not available".to_string())
    }

    /// Connect the selected share and immediately list its root directory.
    pub fn select_share(&self, share: impl Into<String>) -> Result<(), String> {
        self.commands.send(Command::SelectShare(share.into()))
            .map_err(|_| "SMB worker is not available".to_string())
    }

    /// List a directory relative to the currently selected share.
    pub fn list_directory(&self, path: impl Into<String>) -> Result<(), String> {
        self.commands.send(Command::ListDirectory(path.into()))
            .map_err(|_| "SMB worker is not available".to_string())
    }

    pub fn disconnect(&self) -> Result<(), String> {
        self.commands.send(Command::Disconnect)
            .map_err(|_| "SMB worker is not available".to_string())
    }

    /// Read one completed operation without waiting. Returns `None` when no result is ready.
    pub fn try_event(&self) -> Result<Option<Event>, String> {
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("SMB worker stopped unexpectedly".into()),
        }
    }
}

impl Drop for SmbWorker {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        // Do not join here: a network operation may currently be waiting on a remote NAS.
        // Dropping the handle detaches the worker; it exits after that operation returns.
        self.thread.take();
    }
}

/// Friendly wording for common authentication failures, while retaining diagnostic transport errors.
fn user_facing_connect_error(error: String) -> String {
    let lower = error.to_ascii_lowercase();
    let auth_failed = [
        "logon failure",
        "login failure",
        "authentication failed",
        "auth failure",
        "nt_status_logon_failure",
        "status_logon_failure",
        "access denied",
    ].iter().any(|needle| lower.contains(needle));
    if auth_failed {
        "SMB login failed. Check the username and password, or try Guest access.".into()
    } else {
        error
    }
}

#[cfg(test)]
mod connect_error_tests {
    use super::user_facing_connect_error;

    #[test]
    fn common_authentication_errors_are_clear_and_retryable() {
        for error in ["NT_STATUS_LOGON_FAILURE", "Authentication failed", "Access denied", "Login failure"] {
            let friendly = user_facing_connect_error(error.into());
            assert!(friendly.contains("Check the username and password"), "{friendly}");
        }
    }

    #[test]
    fn unrelated_connection_errors_remain_diagnostic() {
        let error = "Connection timed out".to_string();
        assert_eq!(user_facing_connect_error(error.clone()), error);
    }
}

fn worker_loop(commands: Receiver<Command>, events: Sender<Event>) {
    // Keep the validated request so a failed share selection can be retried without forcing
    // the user back through the connection form. The password stays worker-local and Debug-redacted.
    let mut request: Option<ConnectRequest> = None;
    // Keep the authenticated server alive after share enumeration. Selecting the first share
    // should use the connection that just authenticated, not silently authenticate a second time.
    // If selecting a share consumes it and fails, the retained request lets the next attempt reconnect.
    let mut server: Option<crate::Server> = None;
    let mut share: Option<Session> = None;

    while let Ok(command) = commands.recv() {
        match command {
            Command::Connect(incoming) => {
                share = None;
                server = None;
                request = Some(incoming);
                match request.as_ref().expect("request just stored").connect_and_list_shares() {
                    Ok((connected, shares)) => {
                        server = Some(connected);
                        let _ = events.send(Event::Shares(Ok(shares)));
                    }
                    Err(error) => {
                        let _ = events.send(Event::Shares(Err(user_facing_connect_error(error))));
                    }
                }
            }
            Command::SelectShare(name) => {
                // Never leave the previous tree active when the user is switching shares.
                share = None;
                let Some(connect_request) = request.as_ref() else {
                    let _ = events.send(Event::Directory(Err("Connect to an SMB server first".into())));
                    continue;
                };
                // Use the authenticated server from Connect for the first share selection. The
                // server handle is consumed by connect_share; if that operation fails, reconnect
                // from the worker-owned request on the next selection attempt.
                let connected = match server.take() {
                    Some(connected) => connected,
                    None => match connect_request.connect_server() {
                        Ok(connected) => connected,
                        Err(error) => {
                            let _ = events.send(Event::Directory(Err(user_facing_connect_error(error))));
                            continue;
                        }
                    },
                };
                match connected.connect_share_blocking(&name) {
                    Ok(mut connected_share) => match connected_share.list_directory_blocking("") {
                        Ok(entries) => {
                            share = Some(connected_share);
                            let _ = events.send(Event::Directory(Ok(entries)));
                        }
                        Err(error) => {
                            share = Some(connected_share);
                            let _ = events.send(Event::Directory(Err(error)));
                        }
                    },
                    Err(error) => {
                        let _ = events.send(Event::Directory(Err(error)));
                    }
                }
            }
            Command::ListDirectory(path) => {
                let result = match share.as_mut() {
                    Some(connected_share) => connected_share.list_directory_blocking(&path),
                    None => Err("Select an SMB share first".into()),
                };
                let _ = events.send(Event::Directory(result));
            }
            Command::Disconnect => {
                share = None;
                server = None;
                request = None;
                let _ = events.send(Event::Disconnected);
            }
            Command::Shutdown => break,
        }
    }
}
