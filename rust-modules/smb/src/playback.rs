//! Prepare an SMB file for the existing native player without involving a media server.

use crate::{range_http::RangeServer, Session};

/// A prepared direct-play source. Keep the bridge alive until playback has stopped.
pub struct PreparedPlayback {
    pub play: plx_playback_contract::playurl::PlayUrl,
    pub bridge: RangeServer,
    /// Final path component used by the existing player title/caption.
    pub file_name: String,
    /// Probe facts retained for the local playback metadata/track menu.
    pub probe: plx_play_feed::SourceProbe,
}

/// Open one share-relative SMB file, expose it through loopback HTTP Range and inspect the actual
/// codecs with the bundled FFmpeg demuxer. The authenticated session is transferred into RangeServer and disconnected
/// only after the file handle and any in-flight HTTP range handlers have been released.
struct SmbProbeSource {
    reader: std::sync::Arc<smb2::FileReader>,
    size: u64,
}

impl plx_play_feed::ProbeSource for SmbProbeSource {
    fn size(&self) -> u64 { self.size }

    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, String> {
        smol::block_on(self.reader.read_at(offset, length as u64))
            .map_err(|error| format!("SMB random read failed: {error}"))
    }
}

pub fn prepare_playback(mut session: Session, path: &str) -> Result<PreparedPlayback, String> {
    let reader = std::sync::Arc::new(session.open_file_blocking(path)?);
    let source = SmbProbeSource { size: reader.size(), reader: reader.clone() };
    let probe = plx_play_feed::probe_source(std::sync::Arc::new(source))
        .map_err(|error| format!("FFmpeg custom-AVIO probe failed: {error}"))?;
    let bridge = RangeServer::start_shared_with_session(reader, session)?;
    let video_codec = probe
        .video_codec
        .clone()
        .ok_or_else(|| "No video stream found".to_string())?;
    let (profile, compatibility, enhancement) = probe
        .dovi
        .map(|(profile, compatibility, enhancement)| {
            (
                i64::from(profile),
                i64::from(compatibility),
                enhancement,
            )
        })
        .unwrap_or((0, 0, false));

    let play = plx_playback_contract::playurl::PlayUrl {
        url: bridge.url().to_owned(),
        vcodec: video_codec,
        acodec: probe.audio_codec.clone().unwrap_or_default(),
        fps: 0.0,
        dovi: plx_playback_contract::playurl::PlayDovi {
            profile,
            bl_compat: compatibility,
            el_present: enhancement,
        },
        atmos: false,
        auto_source_kbps: 0,
        auto_hls_base: String::new(),
        auto_start_hls: false,
        source_raster: (probe.width > 0 && probe.height > 0)
            .then_some([probe.width, probe.height]),
    };

    let file_name = path.rsplit(|ch| ch == '/' || ch == '\\').find(|part| !part.is_empty()).unwrap_or(path).to_owned();
    Ok(PreparedPlayback { play, bridge, file_name, probe })
}


// SMB sibling playback is an in-memory queue owned by the SMB feature. The cursor
// supports both the player's Previous/Next buttons and automatic advance at end-of-file.
// The existing player returns to the browser for a single frame; that page prepares the
// selected sibling off the frame thread and opens it through the normal playback path.
struct AutoplayState {
    items: Vec<(usize, crate::controller::PlayRequest)>,
    current: Option<usize>,
    armed_target: Option<usize>,
}
static AUTOPLAY: std::sync::OnceLock<std::sync::Mutex<AutoplayState>> = std::sync::OnceLock::new();

fn autoplay_state() -> &'static std::sync::Mutex<AutoplayState> {
    AUTOPLAY.get_or_init(|| std::sync::Mutex::new(AutoplayState {
        items: Vec::new(),
        current: None,
        armed_target: None,
    }))
}

pub fn set_autoplay_queue(items: Vec<(usize, crate::controller::PlayRequest)>, current_row: usize) {
    let mut state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    state.current = items.iter().position(|(row, _)| *row == current_row);
    state.items = items;
    state.armed_target = None;
}

pub fn clear_autoplay_queue() {
    let mut state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    state.items.clear();
    state.current = None;
    state.armed_target = None;
}

pub fn has_autoplay_previous() -> bool {
    let state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    state.current.is_some_and(|current| current > 0)
}

pub fn has_autoplay_next() -> bool {
    let state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    state.current.is_some_and(|current| current + 1 < state.items.len())
}

pub fn arm_autoplay_previous() -> bool {
    let mut state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    let Some(current) = state.current else { return false; };
    let Some(target) = current.checked_sub(1) else { return false; };
    state.armed_target = Some(target);
    true
}

pub fn arm_autoplay_next() -> bool {
    let mut state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    let Some(current) = state.current else { return false; };
    let target = current + 1;
    if target >= state.items.len() {
        state.armed_target = None;
        false
    } else {
        state.armed_target = Some(target);
        true
    }
}

pub fn arm_autoplay_if_next() -> bool {
    arm_autoplay_next()
}

pub fn take_autoplay_next() -> Option<(usize, crate::controller::PlayRequest)> {
    let mut state = autoplay_state().lock().unwrap_or_else(|p| p.into_inner());
    let target = state.armed_target.take()?;
    let item = state.items.get(target)?.clone();
    state.current = Some(target);
    Some(item)
}
