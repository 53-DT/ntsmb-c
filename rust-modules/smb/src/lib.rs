//! Direct SMB2/3 access for the native media browser.
//!
//! `Server` authenticates once and enumerates disk shares; `Session` owns the selected share.
//! This split lets the connection screen offer the server's real share list before browsing.
//!
//! This layer deliberately knows nothing about Plex, Jellyfin, artwork, metadata providers or
//! transcoding. It connects to one SMB share, lists a directory and opens random-access file
//! readers. The player-facing HTTP Range bridge is a separate layer so SMB credentials and
//! protocol details never enter playback URLs. The bridge supports bounded and open-ended
//! byte ranges, allowing the existing FFmpeg demuxer to seek without staging a local copy.

#[cfg(feature = "playback")]
pub mod playback;
pub mod range_http;
pub mod browser;
pub mod profile;
pub mod connect_form;
pub mod worker;
pub mod controller;
pub mod store;
pub mod provider;

use smb2::{FileReader, SmbClient, Tree};

/// One directory row returned by an SMB server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub is_directory: bool,
}

/// One disk share published by an SMB server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInfo {
    pub name: String,
    pub comment: String,
}

/// An authenticated SMB server connection, before a share has been selected.
/// Keep this on a worker thread; share enumeration performs network I/O.
pub struct Server {
    client: SmbClient,
}

impl Server {
    pub async fn connect(host: &str, port: u16, username: &str, password: &str) -> Result<Self, String> {
        let address = endpoint_with_port(host, port)?;
        let client = smb2::connect(&address, username, password)
            .await
            .map_err(|e| format!("SMB connection failed: {e}"))?;
        Ok(Self { client })
    }

    pub async fn list_shares(&mut self) -> Result<Vec<ShareInfo>, String> {
        let shares = self.client.list_shares().await
            .map_err(|e| format!("Could not list SMB shares: {e}"))?;
        Ok(shares.into_iter().map(|share| ShareInfo {
            name: share.name,
            comment: share.comment,
        }).collect())
    }

    pub async fn connect_share(self, share_name: &str) -> Result<Session, String> {
        let share_name = share_name.trim();
        if share_name.is_empty() || share_name.contains('/') || share_name.contains('\\') {
            return Err("Invalid SMB share name".into());
        }
        let mut client = self.client;
        let share = client.connect_share(share_name).await
            .map_err(|e| format!("Could not connect to SMB share: {e}"))?;
        Ok(Session { client, share })
    }

    pub fn list_shares_blocking(&mut self) -> Result<Vec<ShareInfo>, String> {
        smol::block_on(self.list_shares())
    }

    pub fn connect_share_blocking(self, share_name: &str) -> Result<Session, String> {
        smol::block_on(self.connect_share(share_name))
    }
}

pub fn connect_server_blocking(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<Server, String> {
    smol::block_on(Server::connect(host, port, username, password))
}

/// Connect using a validated saved or in-memory NT-SMB profile. Guest mode deliberately supplies
/// the conventional guest identity and an empty password; it never reads a stored credential.
pub fn connect_profile_blocking(
    profile: &profile::ConnectionProfile,
    password: Option<&str>,
) -> Result<Server, String> {
    profile.validate()?;
    let (username, password) = if profile.guest {
        ("guest", "")
    } else {
        let password = password.ok_or_else(|| "Enter the SMB password".to_string())?;
        (profile.username.as_str(), password)
    };
    smol::block_on(Server::connect(&profile.host, profile.port, username, password))
}

/// Reconnect from a saved profile. The password is opened only through the platform's secure
/// credential store; missing, unsupported or unreadable credentials produce a prompt-worthy
/// error rather than attempting an empty or plaintext password.
pub fn connect_saved_profile_blocking(
    profile: &profile::ConnectionProfile,
) -> Result<Server, String> {
    profile.validate()?;
    if profile.guest {
        return connect_profile_blocking(profile, None);
    }
    let password = profile.reveal_password()
        .ok_or_else(|| "Saved SMB password is unavailable. Please enter it again.".to_string())?;
    connect_profile_blocking(profile, Some(&password))
}

/// An authenticated connection to one share.
///
/// The SMB client and share tree are kept together because a tree is scoped to its session.
/// Callers should keep this object on a worker thread and never call it from the render/frame
/// thread. smb2 performs asynchronous network I/O; the app's worker may drive these futures with
/// smol::block_on.
pub struct Session {
    client: SmbClient,
    share: Tree,
}

impl Session {
    /// Connect to host[:port] (port 445 when omitted), authenticate and connect to a share.
    /// The password is passed directly to the SMB authentication layer and is never formatted
    /// into an error or URL by this module.
    pub async fn connect(
        host: &str,
        username: &str,
        password: &str,
        share_name: &str,
    ) -> Result<Self, String> {
        Self::connect_address(endpoint(host)?, username, password, share_name).await
    }

    /// Connect with a separately supplied TCP port, as used by the NT-SMB connection form.
    /// The host field contains only a hostname or IP address; the port is validated independently.
    pub async fn connect_on_port(
        host: &str,
        port: u16,
        username: &str,
        password: &str,
        share_name: &str,
    ) -> Result<Self, String> {
        let address = endpoint_with_port(host, port)?;
        Self::connect_address(address, username, password, share_name).await
    }

    async fn connect_address(
        address: String,
        username: &str,
        password: &str,
        share_name: &str,
    ) -> Result<Self, String> {
        let share_name = share_name.trim();
        if share_name.is_empty() || share_name.contains('/') || share_name.contains('\\') {
            return Err("Invalid SMB share name".into());
        }

        let mut client = smb2::connect(&address, username, password)
            .await
            .map_err(|e| format!("SMB connection failed: {e}"))?;
        let share = client
            .connect_share(share_name)
            .await
            .map_err(|e| format!("Could not connect to SMB share: {e}"))?;

        Ok(Self { client, share })
    }

    /// List one directory relative to the connected share root. Empty string means share root.
    pub async fn list_directory(&mut self, path: &str) -> Result<Vec<Entry>, String> {
        let path = share_path(path, true)?;
        let rows = self
            .client
            .list_directory(&mut self.share, &path)
            .await
            .map_err(|e| format!("Could not list SMB directory: {e}"))?;

        Ok(rows
            .into_iter()
            .filter(|row| row.name != "." && row.name != "..")
            .map(|row| Entry {
                name: row.name,
                size: row.size,
                is_directory: row.is_directory,
            })
            .collect())
    }

    /// Open a file for positioned reads. The returned reader owns the underlying SMB handle
    /// and can service seeks without downloading the complete file.
    pub async fn open_file(&mut self, path: &str) -> Result<FileReader, String> {
        let path = share_path(path, false)?;
        self.client
            .open_file_reader(&self.share, &path)
            .await
            .map_err(|e| format!("Could not open SMB file: {e}"))
    }

    /// Blocking worker-thread convenience; never call from the render/frame thread.
    pub fn list_directory_blocking(&mut self, path: &str) -> Result<Vec<Entry>, String> {
        smol::block_on(self.list_directory(path))
    }

    /// Blocking worker-thread convenience; never call from the render/frame thread.
    pub fn open_file_blocking(&mut self, path: &str) -> Result<FileReader, String> {
        smol::block_on(self.open_file(path))
    }

    /// Disconnect a share cleanly. Call after readers have been closed.
    pub async fn disconnect(mut self) -> Result<(), String> {
        self.client
            .disconnect_share(&self.share)
            .await
            .map_err(|e| format!("Could not disconnect SMB share: {e}"))
    }

    /// Blocking worker-thread convenience for clean disconnect.
    pub fn disconnect_blocking(self) -> Result<(), String> {
        smol::block_on(self.disconnect())
    }
}

/// Connect and authenticate using the app's worker-thread execution model.
pub fn connect_blocking(host: &str, username: &str, password: &str, share_name: &str) -> Result<Session, String> {
    smol::block_on(Session::connect(host, username, password, share_name))
}

/// Worker-thread convenience for the separate host/port fields in the SMB form.
pub fn connect_blocking_on_port(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    share_name: &str,
) -> Result<Session, String> {
    smol::block_on(Session::connect_on_port(host, port, username, password, share_name))
}

/// Build a TCP endpoint from a hostname/IP and a separately supplied port.
fn endpoint_with_port(host: &str, port: u16) -> Result<String, String> {
    if port == 0 {
        return Err("Invalid SMB server port".into());
    }
    let host = host.trim();
    if host.is_empty()
        || host.contains("://")
        || host.contains('/')
        || host.contains('\\')
        || host.chars().any(char::is_whitespace)
    {
        return Err("Invalid SMB server address".into());
    }

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(match ip {
            std::net::IpAddr::V4(_) => format!("{ip}:{port}"),
            std::net::IpAddr::V6(_) => format!("[{ip}]:{port}"),
        });
    }

    if host.starts_with('[') && host.ends_with(']') {
        let inner = &host[1..host.len() - 1];
        if inner.parse::<std::net::Ipv6Addr>().is_ok() {
            return Ok(format!("[{inner}]:{port}"));
        }
    }

    if host.contains(':') || host.starts_with('[') || host.ends_with(']') {
        return Err("Enter the host without a port; use the Port field".into());
    }
    Ok(format!("{host}:{port}"))
}

/// Turn a host or host:port into the TCP endpoint smb2 expects.
fn endpoint(host: &str) -> Result<String, String> {
    let host = host.trim();
    if host.is_empty()
        || host.contains("://")
        || host.contains('/')
        || host.contains('\\')
        || host.chars().any(char::is_whitespace)
    {
        return Err("Invalid SMB server address".into());
    }

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(match ip {
            std::net::IpAddr::V4(_) => format!("{ip}:445"),
            std::net::IpAddr::V6(_) => format!("[{ip}]:445"),
        });
    }

    if host.starts_with('[') {
        let end = host.find(']').ok_or("Invalid bracketed IPv6 address")?;
        let suffix = &host[end + 1..];
        if suffix.is_empty() {
            return Ok(format!("{host}:445"));
        }
        if suffix.starts_with(':') && suffix[1..].parse::<u16>().is_ok() {
            return Ok(host.to_owned());
        }
        return Err("Invalid SMB server port".into());
    }

    if let Some((name, port)) = host.rsplit_once(':') {
        if !name.is_empty() && port.parse::<u16>().is_ok() {
            return Ok(host.to_owned());
        }
        return Err("Invalid SMB server port".into());
    }

    Ok(format!("{host}:445"))
}

/// Validate and normalize a share-relative path. Traversal above the share root is refused.
fn share_path(path: &str, allow_root: bool) -> Result<String, String> {
    if path.contains('\0') || path.contains('\\') {
        return Err("Invalid SMB path".into());
    }

    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("Path cannot leave the SMB share".into()),
            value => parts.push(value),
        }
    }

    if parts.is_empty() && !allow_root {
        return Err("An SMB file path is required".into());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::{endpoint, endpoint_with_port, share_path};

    #[test]
    fn endpoint_defaults_to_port_445() {
        assert_eq!(endpoint("nas.local").unwrap(), "nas.local:445");
        assert_eq!(endpoint("192.168.1.20").unwrap(), "192.168.1.20:445");
        assert_eq!(endpoint("2001:db8::1").unwrap(), "[2001:db8::1]:445");
    }

    #[test]
    fn separate_port_field_supports_host_ipv4_and_ipv6() {
        assert_eq!(endpoint_with_port("nas.local", 1445).unwrap(), "nas.local:1445");
        assert_eq!(endpoint_with_port("192.168.1.20", 445).unwrap(), "192.168.1.20:445");
        assert_eq!(endpoint_with_port("2001:db8::1", 1445).unwrap(), "[2001:db8::1]:1445");
        assert_eq!(endpoint_with_port("[2001:db8::1]", 445).unwrap(), "[2001:db8::1]:445");
    }

    #[test]
    fn separate_port_field_rejects_invalid_port_and_embedded_port() {
        assert!(endpoint_with_port("nas.local", 0).is_err());
        assert!(endpoint_with_port("nas.local:1445", 445).is_err());
        assert!(endpoint_with_port("smb://nas.local", 445).is_err());
    }

    #[test]
    fn endpoint_preserves_explicit_ports() {
        assert_eq!(endpoint("nas.local:1445").unwrap(), "nas.local:1445");
        assert_eq!(endpoint("[2001:db8::1]:1445").unwrap(), "[2001:db8::1]:1445");
    }

    #[test]
    fn endpoint_rejects_urls_and_malformed_addresses() {
        assert!(endpoint("smb://nas/share").is_err());
        assert!(endpoint("nas.local:nope").is_err());
        assert!(endpoint("nas.local/share").is_err());
    }

    #[test]
    fn share_paths_are_relative_and_normalized() {
        assert_eq!(share_path("", true).unwrap(), "");
        assert_eq!(share_path("/Movies//Films/./", true).unwrap(), "Movies/Films");
        assert_eq!(share_path("Movies/Film.mkv", false).unwrap(), "Movies/Film.mkv");
    }

    #[test]
    fn share_paths_cannot_escape_the_share() {
        assert!(share_path("../secret.mkv", false).is_err());
        assert!(share_path("Movies/../../secret.mkv", false).is_err());
        assert!(share_path("", false).is_err());
    }
}
