//! Provider-neutral source contract and concrete SMB/local adapters for DeckFiles.
use crate::{connect_server_blocking, Entry, Session};
use plx_os::storage::deck_vault::{DeckVault, SourceRecord};
pub use plx_os::storage::deck_vault::SourceKind;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind { Folder, Video, Subtitle, Other }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub kind: EntryKind,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ConnectPayload {
    pub kind: SourceKind,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub share: String,
    pub username: String,
    pub guest: bool,
    pub secret: Option<String>,
    pub root_path: String,
}

#[cfg(feature = "playback")]
pub enum PreparedSourcePlay {
    Smb {
        prepared: crate::playback::PreparedPlayback,
        playlist_paths: Vec<String>,
        matched_sidecars: Vec<String>,
    },
    Local {
        play: plx_playback_contract::playurl::PlayUrl,
        playlist_paths: Vec<String>,
        matched_sidecars: Vec<String>,
    },
}

pub trait SourceProvider: Send + Sync {
    fn kind(&self) -> SourceKind;
    fn display_name(&self) -> &'static str;
    fn available(&self) -> bool { true }
    fn connect(&self, payload: &ConnectPayload, vault: &mut DeckVault) -> Result<SourceRecord, String>;
    fn list_dir(&self, source: &SourceRecord, rel_path: &str, vault: &DeckVault) -> Result<Vec<MediaEntry>, String>;
    #[cfg(feature = "playback")]
    fn prepare_play(&self, source: &SourceRecord, file_path: &str, vault: &DeckVault) -> Result<PreparedSourcePlay, String>;
}

pub struct SmbProvider;
impl SmbProvider {
    pub fn new() -> Self { Self }
    fn credentials(source: &SourceRecord, vault: &DeckVault) -> Result<(String, String), String> {
        if source.guest { return Ok(("guest".into(), String::new())); }
        let secret = vault.unseal_secret(&source.id).ok_or_else(|| "Saved SMB credential is unavailable; reconnect and enter it again".to_string())?;
        Ok((source.username.clone(), secret))
    }
    fn connect_share(source: &SourceRecord, vault: &DeckVault) -> Result<Session, String> {
        if source.share.trim().is_empty() { return Err("Choose an SMB share before browsing".into()); }
        let (user, pass) = Self::credentials(source, vault)?;
        connect_server_blocking(&source.host, source.port, &user, &pass)?.connect_share_blocking(&source.share)
    }
}
impl Default for SmbProvider { fn default() -> Self { Self::new() } }

impl SourceProvider for SmbProvider {
    fn kind(&self) -> SourceKind { SourceKind::Smb }
    fn display_name(&self) -> &'static str { "SMB" }

    fn connect(&self, payload: &ConnectPayload, vault: &mut DeckVault) -> Result<SourceRecord, String> {
        let host = payload.host.trim();
        if host.is_empty() || payload.port == 0 { return Err("Enter a valid SMB host and port".into()); }
        if !payload.guest && payload.username.trim().is_empty() { return Err("Enter an SMB username or enable Guest".into()); }
        let username = if payload.guest { "guest" } else { payload.username.trim() };
        let secret = if payload.guest { "" } else { payload.secret.as_deref().ok_or_else(|| "Enter the SMB password".to_string())? };
        let mut server = connect_server_blocking(host, payload.port, username, secret)?;
        if !payload.share.trim().is_empty() {
            let mut session = server.connect_share_blocking(&payload.share)?;
            session.list_directory_blocking("")?;
            session.disconnect_blocking()?;
        } else {
            // Authentication is verified even when the user will choose a share in the next step.
            let _ = server.list_shares_blocking()?;
        }
        let id = format!("smb:{}:{}/{}:{}", host.to_ascii_lowercase(), payload.port, payload.share.trim().to_ascii_lowercase(), username);
        let now = unix_now();
        let record = SourceRecord { id, kind: SourceKind::Smb, label: nonempty(&payload.label, host), host: host.into(),
            port: payload.port, share: payload.share.trim().into(), username: username.into(), guest: payload.guest,
            has_saved_credential: false, root_path: String::new(), last_path: String::new(), created_at: now, last_connected_at: now };
        vault.upsert_source(record.clone(), if payload.guest { None } else { payload.secret.as_deref() })?;
        Ok(record)
    }

    fn list_dir(&self, source: &SourceRecord, rel_path: &str, vault: &DeckVault) -> Result<Vec<MediaEntry>, String> {
        let mut session = Self::connect_share(source, vault)?;
        let rows = session.list_directory_blocking(rel_path)?;
        let mut result = rows.into_iter().map(|entry| media_entry(entry, rel_path)).collect::<Vec<_>>();
        sort_entries(&mut result);
        Ok(result)
    }

    #[cfg(feature = "playback")]
    fn prepare_play(&self, source: &SourceRecord, file_path: &str, vault: &DeckVault) -> Result<PreparedSourcePlay, String> {
        let mut session = Self::connect_share(source, vault)?;
        let parent = file_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        let name = file_path.rsplit('/').next().unwrap_or(file_path);
        let entries = session.list_directory_blocking(parent)?;
        let (playlist_paths, matched_sidecars) = playlist_context(&entries, parent, name);
        let prepared = crate::playback::prepare_playback(session, file_path)?;
        Ok(PreparedSourcePlay::Smb { prepared, playlist_paths, matched_sidecars })
    }
}

pub struct LocalProvider;
impl LocalProvider {
    pub fn new() -> Self { Self }
    fn root(source: &SourceRecord) -> Result<PathBuf, String> {
        let root = PathBuf::from(&source.root_path).canonicalize().map_err(|_| "Local media folder is unavailable".to_string())?;
        if !root.is_dir() { return Err("Local media source is not a folder".into()); }
        Ok(root)
    }
    fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
        if rel.contains('\0') || rel.contains('\\') || Path::new(rel).components().any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_))) {
            return Err("Invalid local media path".into());
        }
        let path = root.join(rel).canonicalize().map_err(|_| "Local media path is unavailable".to_string())?;
        if !path.starts_with(root) { return Err("Local media path leaves the selected folder".into()); }
        Ok(path)
    }
}
impl Default for LocalProvider { fn default() -> Self { Self::new() } }
impl SourceProvider for LocalProvider {
    fn kind(&self) -> SourceKind { SourceKind::Local }
    fn display_name(&self) -> &'static str { "Local Media" }

    fn connect(&self, payload: &ConnectPayload, vault: &mut DeckVault) -> Result<SourceRecord, String> {
        let root = PathBuf::from(payload.root_path.trim()).canonicalize().map_err(|_| "Choose an existing local media folder".to_string())?;
        if !root.is_dir() { return Err("Choose an existing local media folder".into()); }
        let root_path = root.to_string_lossy().into_owned();
        let now = unix_now();
        let record = SourceRecord { id: format!("local:{root_path}"), kind: SourceKind::Local,
            label: nonempty(&payload.label, "Local Media"), host: String::new(), port: 0, share: String::new(),
            username: String::new(), guest: true, has_saved_credential: false, root_path, last_path: String::new(),
            created_at: now, last_connected_at: now };
        vault.upsert_source(record.clone(), None)?;
        Ok(record)
    }

    fn list_dir(&self, source: &SourceRecord, rel_path: &str, _vault: &DeckVault) -> Result<Vec<MediaEntry>, String> {
        let root = Self::root(source)?;
        let directory = if rel_path.is_empty() { root.clone() } else { Self::resolve(&root, rel_path)? };
        if !directory.is_dir() { return Err("Selected local path is not a folder".into()); }
        let mut rows = Vec::new();
        for item in std::fs::read_dir(directory).map_err(|_| "Could not read local media folder".to_string())? {
            let item = item.map_err(|_| "Could not read local media entry".to_string())?;
            let ty = item.file_type().map_err(|_| "Could not inspect local media entry".to_string())?;
            if ty.is_symlink() { continue; }
            let name = item.file_name().to_string_lossy().into_owned();
            let metadata = item.metadata().map_err(|_| "Could not inspect local media entry".to_string())?;
            rows.push(media_entry(Entry { name, size: metadata.len(), is_directory: ty.is_dir() }, rel_path));
        }
        sort_entries(&mut rows);
        Ok(rows)
    }

    #[cfg(feature = "playback")]
    fn prepare_play(&self, source: &SourceRecord, file_path: &str, _vault: &DeckVault) -> Result<PreparedSourcePlay, String> {
        use std::os::unix::fs::FileExt;
        let root = Self::root(source)?;
        let path = Self::resolve(&root, file_path)?;
        if !path.is_file() { return Err("Selected local media item is not a file".into()); }
        let file = std::fs::File::open(&path).map_err(|_| "Could not open local media file".to_string())?;
        let size = file.metadata().map_err(|_| "Could not inspect local media file".to_string())?.len();
        struct FileProbe { file: std::fs::File, size: u64 }
        impl plx_play_feed::ProbeSource for FileProbe {
            fn size(&self) -> u64 { self.size }
            fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, String> {
                let mut data = vec![0; length];
                let count = self.file.read_at(&mut data, offset).map_err(|_| "Local media read failed".to_string())?;
                data.truncate(count); Ok(data)
            }
        }
        let probe = plx_play_feed::probe_source(std::sync::Arc::new(FileProbe { file, size }))
            .map_err(|e| format!("FFmpeg local-file probe failed: {e}"))?;
        let video_codec = probe.video_codec.clone().ok_or_else(|| "No video stream found".to_string())?;
        let (profile, compatibility, enhancement) = probe.dovi.map(|(p,c,e)| (i64::from(p),i64::from(c),e)).unwrap_or((0,0,false));
        let play = plx_playback_contract::playurl::PlayUrl { url: path.to_string_lossy().into_owned(), vcodec: video_codec,
            acodec: probe.audio_codec.clone().unwrap_or_default(), fps: 0.0,
            dovi: plx_playback_contract::playurl::PlayDovi { profile, bl_compat: compatibility, el_present: enhancement },
            atmos: false, auto_source_kbps: 0, auto_hls_base: String::new(), auto_start_hls: false,
            source_raster: (probe.width > 0 && probe.height > 0).then_some([probe.width, probe.height]) };
        let parent = file_path.rsplit_once('/').map(|(p,_)| p).unwrap_or("");
        let name = file_path.rsplit('/').next().unwrap_or(file_path);
        let directory = if parent.is_empty() { root.clone() } else { Self::resolve(&root, parent)? };
        let entries = std::fs::read_dir(directory).map_err(|_| "Could not read local media folder".to_string())?
            .filter_map(Result::ok).filter_map(|entry| {
                let ty = entry.file_type().ok()?;
                if ty.is_symlink() { return None; }
                let metadata = entry.metadata().ok()?;
                Some(Entry { name: entry.file_name().to_string_lossy().into_owned(), size: metadata.len(), is_directory: ty.is_dir() })
            }).collect::<Vec<_>>();
        let (playlist_paths, matched_sidecars) = playlist_context(&entries, parent, name);
        Ok(PreparedSourcePlay::Local { play, playlist_paths, matched_sidecars })
    }
}

/// Providers are deliberately registered in one place; unavailable cloud slots do not pretend to connect.
pub struct GoogleDriveSlot;
impl SourceProvider for GoogleDriveSlot {
    fn kind(&self) -> SourceKind { SourceKind::GoogleDriveSlot }
    fn display_name(&self) -> &'static str { "Google Drive" }
    fn available(&self) -> bool { false }
    fn connect(&self, _: &ConnectPayload, _: &mut DeckVault) -> Result<SourceRecord,String> { Err("Google Drive provider is not installed yet".into()) }
    fn list_dir(&self, _: &SourceRecord, _: &str, _: &DeckVault) -> Result<Vec<MediaEntry>,String> { Err("Google Drive provider is not installed yet".into()) }
    #[cfg(feature = "playback")]
    fn prepare_play(&self, _: &SourceRecord, _: &str, _: &DeckVault) -> Result<PreparedSourcePlay,String> { Err("Google Drive provider is not installed yet".into()) }
}

pub struct WebDavSlot;
impl SourceProvider for WebDavSlot {
    fn kind(&self) -> SourceKind { SourceKind::WebDavSlot }
    fn display_name(&self) -> &'static str { "WebDAV" }
    fn available(&self) -> bool { false }
    fn connect(&self, _: &ConnectPayload, _: &mut DeckVault) -> Result<SourceRecord,String> { Err("WebDAV provider is not installed yet".into()) }
    fn list_dir(&self, _: &SourceRecord, _: &str, _: &DeckVault) -> Result<Vec<MediaEntry>,String> { Err("WebDAV provider is not installed yet".into()) }
    #[cfg(feature = "playback")]
    fn prepare_play(&self, _: &SourceRecord, _: &str, _: &DeckVault) -> Result<PreparedSourcePlay,String> { Err("WebDAV provider is not installed yet".into()) }
}

pub struct SourceRegistry { providers: Vec<Box<dyn SourceProvider>> }
impl SourceRegistry {
    pub fn new() -> Self { Self { providers: vec![Box::new(SmbProvider), Box::new(LocalProvider), Box::new(GoogleDriveSlot), Box::new(WebDavSlot)] } }
    pub fn get(&self, kind: SourceKind) -> Option<&dyn SourceProvider> { self.providers.iter().find(|p| p.kind() == kind).map(|p| p.as_ref()) }
    pub fn providers(&self) -> impl Iterator<Item=&dyn SourceProvider> { self.providers.iter().map(|p| p.as_ref()) }
}
impl Default for SourceRegistry { fn default() -> Self { Self::new() } }

fn media_entry(entry: Entry, parent: &str) -> MediaEntry {
    let kind = if entry.is_directory { EntryKind::Folder } else { match extension(&entry.name).as_str() {
        "mp4"|"mkv"|"mov"|"avi"|"m4v"|"webm"|"ts"|"m2ts"|"mts"|"mpg"|"mpeg"|"wmv"|"mxf" => EntryKind::Video,
        "srt"|"vtt"|"ass"|"ssa" => EntryKind::Subtitle, _ => EntryKind::Other } };
    let path = if parent.trim_matches('/').is_empty() { entry.name.clone() } else { format!("{}/{}", parent.trim_matches('/'), entry.name) };
    MediaEntry { name: entry.name, path, is_dir: entry.is_directory, kind, size_bytes: entry.size }
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
                let az = an.trim_start_matches('0'); let bz = bn.trim_start_matches('0');
                let order = az.len().cmp(&bz.len()).then_with(|| az.cmp(bz)).then_with(|| an.len().cmp(&bn.len()));
                if order != std::cmp::Ordering::Equal { return order; }
            }
            (Some(x), Some(y)) => {
                let order = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
                a.next(); b.next();
                if order != std::cmp::Ordering::Equal { return order; }
            }
        }
    }
}
fn extension(name: &str) -> String { name.rsplit_once('.').map(|(_,e)| e.to_ascii_lowercase()).unwrap_or_default() }
fn sort_entries(rows: &mut [MediaEntry]) {
    rows.sort_by(|a,b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)).then_with(|| a.name.cmp(&b.name)));
}
fn playlist_context(entries: &[Entry], parent: &str, current: &str) -> (Vec<String>, Vec<String>) {
    let stem = current.rsplit_once('.').map(|(s,_)| s).unwrap_or(current).to_lowercase();
    let mut videos = entries.iter().filter(|e| !e.is_directory && matches!(extension(&e.name).as_str(),
        "mp4"|"mkv"|"mov"|"avi"|"m4v"|"webm"|"ts"|"m2ts"|"mts"|"mpg"|"mpeg"|"wmv"|"mxf"))
        .map(|e| if parent.is_empty() { e.name.clone() } else { format!("{}/{}",parent,e.name) }).collect::<Vec<_>>();
    videos.sort_by(|a,b| a.to_lowercase().cmp(&b.to_lowercase()));
    let sidecars = entries.iter().filter(|e| !e.is_directory && matches!(extension(&e.name).as_str(),"srt"|"vtt"|"ass")
        && e.name.rsplit_once('.').map(|(s,_)| s.to_lowercase()) == Some(stem.clone()))
        .map(|e| if parent.is_empty() { e.name.clone() } else { format!("{}/{}",parent,e.name) }).collect();
    (videos, sidecars)
}
fn nonempty(value: &str, fallback: &str) -> String { if value.trim().is_empty() { fallback.into() } else { value.trim().into() } }
fn unix_now() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classifies_video_subtitle_and_other_entries() {
        assert_eq!(media_entry(Entry{name:"Film.MKV".into(),size:1,is_directory:false},"").kind, EntryKind::Video);
        assert_eq!(media_entry(Entry{name:"Film.srt".into(),size:1,is_directory:false},"").kind, EntryKind::Subtitle);
        assert_eq!(media_entry(Entry{name:"cover.jpg".into(),size:1,is_directory:false},"").kind, EntryKind::Other);
    }
    #[test]
    fn sorts_directories_first_then_case_insensitive_name() {
        let mut rows=vec![media_entry(Entry{name:"b.mkv".into(),size:1,is_directory:false},""),
            media_entry(Entry{name:"Zoo".into(),size:0,is_directory:true},""),
            media_entry(Entry{name:"A.mkv".into(),size:1,is_directory:false},"")];
        sort_entries(&mut rows);
        assert_eq!(rows.iter().map(|r|r.name.as_str()).collect::<Vec<_>>(),["Zoo","A.mkv","b.mkv"]);
    }
}
