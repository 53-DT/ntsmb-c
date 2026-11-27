//! Loopback HTTP Range bridge for an SMB file.
//!
//! The existing player already understands HTTP byte ranges. This bridge binds only to
//! 127.0.0.1 and maps one unguessable local URL to one SMB FileReader. It does not expose the
//! SMB server, share name, username or password in the playback URL.

use smb2::FileReader;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const IO_TIMEOUT: Duration = Duration::from_secs(15);
const ACCEPT_POLL: Duration = Duration::from_millis(40);
const READ_CHUNK: u64 = 1024 * 1024;
const MAX_HEADER: usize = 8192;

/// A local, range-capable HTTP URL backed by one open SMB file handle.
///
/// Keep this value alive for the whole playback session. Call stop when playback ends; Drop also
/// stops the listener and closes the SMB reader. The URL is loopback-only and contains a random
/// 128-bit token, never SMB credentials or a remote share path.
pub struct RangeServer {
    url: String,
    stop: Arc<AtomicBool>,
    stats: Arc<BridgeStats>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct BridgeStats {
    accepted: AtomicU64,
    requests: AtomicU64,
    get_requests: AtomicU64,
    head_requests: AtomicU64,
    bad_requests: AtomicU64,
    bad_paths: AtomicU64,
    bad_ranges: AtomicU64,
    read_errors: AtomicU64,
    bytes_served: AtomicU64,
}

impl BridgeStats {
    fn summary(&self) -> String {
        format!("accepted={} requests={} GET={} HEAD={} bad_request={} bad_path={} bad_range={} read_errors={} bytes={}",
            self.accepted.load(Ordering::Relaxed), self.requests.load(Ordering::Relaxed),
            self.get_requests.load(Ordering::Relaxed), self.head_requests.load(Ordering::Relaxed),
            self.bad_requests.load(Ordering::Relaxed), self.bad_paths.load(Ordering::Relaxed),
            self.bad_ranges.load(Ordering::Relaxed), self.read_errors.load(Ordering::Relaxed),
            self.bytes_served.load(Ordering::Relaxed))
    }
}

impl RangeServer {
    /// Start serving the already-open SMB reader on an ephemeral loopback port.
    pub fn start(reader: FileReader) -> Result<Self, String> {
        Self::start_inner(Arc::new(reader), None)
    }

    /// Start serving a reader and retain its authenticated SMB session until playback stops.
    /// The share is disconnected only after active HTTP handlers and the file reader are closed.
    pub fn start_with_session(reader: FileReader, session: crate::Session) -> Result<Self, String> {
        Self::start_inner(Arc::new(reader), Some(session))
    }

    /// Start serving a reader already shared with the FFmpeg custom-AVIO probe.
    pub fn start_shared_with_session(reader: Arc<FileReader>, session: crate::Session) -> Result<Self, String> {
        Self::start_inner(reader, Some(session))
    }

    fn start_inner(reader: Arc<FileReader>, session: Option<crate::Session>) -> Result<Self, String> {
        let token = random_token()?;
        let size = reader.size();

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("Could not start local playback bridge: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("Could not configure local playback bridge: {e}"))?;
        let address = listener
            .local_addr()
            .map_err(|e| format!("Could not read local playback address: {e}"))?;
        let url = format!("http://127.0.0.1:{}/{token}", address.port());
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(BridgeStats::default());
        let thread_stop = stop.clone();
        let thread_stats = stats.clone();

        let thread = thread::Builder::new()
            .name("smb-range-http".into())
            .spawn(move || serve(listener, address, token, size, reader, session, thread_stop, thread_stats))
            .map_err(|e| format!("Could not start local playback worker: {e}"))?;

        Ok(Self {
            url,
            stop,
            stats,
            thread: Some(thread),
        })
    }

    /// The HTTP URL the existing player can open.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn diagnostics(&self) -> String {
        self.stats.summary()
    }

    /// Stop accepting requests, finish active handlers and close the SMB file handle.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RangeServer {
    fn drop(&mut self) {
        // The frame thread may own this value during playback teardown. Signal the listener and
        // detach its join handle; the worker finishes active requests and closes the SMB handle.
        self.stop.store(true, Ordering::Release);
        let _ = self.thread.take();
    }
}

fn serve(
    listener: TcpListener,
    _address: SocketAddr,
    token: String,
    size: u64,
    reader: Arc<FileReader>,
    session: Option<crate::Session>,
    stop: Arc<AtomicBool>,
    stats: Arc<BridgeStats>,
) {
    let mut handlers: Vec<JoinHandle<()>> = Vec::new();

    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _peer)) => {
                stats.accepted.fetch_add(1, Ordering::Relaxed);
                let reader = reader.clone();
                let token = token.clone();
                let stats = stats.clone();
                handlers.push(thread::spawn(move || handle(stream, &token, size, reader, stats)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => break,
        }

        let mut i = 0;
        while i < handlers.len() {
            if handlers[i].is_finished() {
                let finished = handlers.swap_remove(i);
                let _ = finished.join();
            } else {
                i += 1;
            }
        }
    }

    for handler in handlers {
        let _ = handler.join();
    }

    if let Ok(reader) = Arc::try_unwrap(reader) {
        let _ = smol::block_on(reader.close());
    }
    if let Some(session) = session {
        let _ = session.disconnect_blocking();
    }
}

fn handle(mut stream: TcpStream, token: &str, size: u64, reader: Arc<FileReader>, stats: Arc<BridgeStats>) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            stats.bad_requests.fetch_add(1, Ordering::Relaxed);
            let _ = response(&mut stream, 400, "Bad Request", &[], None);
            return;
        }
    };
    stats.requests.fetch_add(1, Ordering::Relaxed);
    match request.method.as_str() {
        "GET" => { stats.get_requests.fetch_add(1, Ordering::Relaxed); }
        "HEAD" => { stats.head_requests.fetch_add(1, Ordering::Relaxed); }
        _ => {}
    }

    if request.target != format!("/{token}") {
        stats.bad_paths.fetch_add(1, Ordering::Relaxed);
        let _ = response(&mut stream, 404, "Not Found", &[], None);
        return;
    }
    if request.method != "GET" && request.method != "HEAD" {
        let _ = response(&mut stream, 405, "Method Not Allowed", &[("Allow", "GET, HEAD".into())], None);
        return;
    }

    let range_header = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("range"))
        .map(|(_, value)| value.as_str());

    let (start, end, partial) = match requested_range(range_header, size) {
        Ok(range) => range,
        Err(()) => {
            stats.bad_ranges.fetch_add(1, Ordering::Relaxed);
            let extra = [("Content-Range", format!("bytes */{size}"))];
            let _ = response(&mut stream, 416, "Range Not Satisfiable", &extra, Some(0));
            return;
        }
    };

    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut extra = vec![
        ("Accept-Ranges", "bytes".into()),
        ("Content-Type", "application/octet-stream".into()),
        ("Cache-Control", "no-store".into()),
    ];
    if partial {
        extra.push(("Content-Range", format!("bytes {start}-{end}/{size}")));
    }

    let (status, reason) = if partial { (206, "Partial Content") } else { (200, "OK") };
    if response(&mut stream, status, reason, &extra, Some(length)).is_err() {
        return;
    }
    if request.method == "HEAD" || length == 0 {
        return;
    }

    let mut offset = start;
    while offset <= end {
        let wanted = (end - offset + 1).min(READ_CHUNK);
        let bytes = match smol::block_on(reader.read_at(offset, wanted)) {
            Ok(bytes) => bytes,
            Err(_) => {
                stats.read_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if bytes.is_empty() {
            stats.read_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if stream.write_all(&bytes).is_err() {
            stats.read_errors.fetch_add(1, Ordering::Relaxed);
            return;
        }
        stats.bytes_served.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        offset += bytes.len() as u64;
        if bytes.len() as u64 != wanted {
            return;
        }
    }
}

struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

fn read_request(stream: &mut TcpStream) -> Result<Request, ()> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        if bytes.len() >= MAX_HEADER {
            return Err(());
        }
        let read = stream.read(&mut chunk).map_err(|_| ())?;
        if read == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..read]);
        if find_header_end(&bytes).is_some() {
            break;
        }
    }

    let end = find_header_end(&bytes).ok_or(())?;
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().ok_or(())?.split_whitespace();
    let method = request_line.next().ok_or(())?.to_owned();
    let target = request_line.next().ok_or(())?.to_owned();
    let version = request_line.next().ok_or(())?;
    if !version.starts_with("HTTP/1.") || request_line.next().is_some() {
        return Err(());
    }

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').ok_or(())?;
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }

    Ok(Request { method, target, headers })
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn requested_range(header: Option<&str>, size: u64) -> Result<(u64, u64, bool), ()> {
    if size == 0 {
        return if header.is_none() { Ok((0, 0, false)) } else { Err(()) };
    }
    let Some(header) = header else {
        return Ok((0, size - 1, false));
    };

    let value = header.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') {
        return Err(());
    }
    let (first, last) = value.split_once('-').ok_or(())?;

    if first.is_empty() {
        let suffix = last.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = size.saturating_sub(suffix);
        return Ok((start, size - 1, true));
    }

    let start = first.parse::<u64>().map_err(|_| ())?;
    if start >= size {
        return Err(());
    }
    let end = if last.is_empty() {
        size - 1
    } else {
        last.parse::<u64>().map_err(|_| ())?.min(size - 1)
    };
    if end < start {
        return Err(());
    }
    Ok((start, end, true))
}

fn response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    extra: &[(&str, String)],
    length: Option<u64>,
) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    if let Some(length) = length {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    for (name, value) in extra {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|e| format!("Could not create a private playback token: {e}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::requested_range;

    #[test]
    fn full_file_request_uses_status_200_range() {
        assert_eq!(requested_range(None, 100).unwrap(), (0, 99, false));
    }

    #[test]
    fn open_ended_and_closed_ranges_are_supported() {
        assert_eq!(requested_range(Some("bytes=40-"), 100).unwrap(), (40, 99, true));
        assert_eq!(requested_range(Some("bytes=10-19"), 100).unwrap(), (10, 19, true));
    }

    #[test]
    fn suffix_ranges_are_supported() {
        assert_eq!(requested_range(Some("bytes=-20"), 100).unwrap(), (80, 99, true));
        assert_eq!(requested_range(Some("bytes=-200"), 100).unwrap(), (0, 99, true));
    }

    #[test]
    fn invalid_and_unsatisfiable_ranges_are_rejected() {
        assert!(requested_range(Some("items=1-2"), 100).is_err());
        assert!(requested_range(Some("bytes=100-"), 100).is_err());
        assert!(requested_range(Some("bytes=20-10"), 100).is_err());
        assert!(requested_range(Some("bytes=0-1,4-5"), 100).is_err());
        assert!(requested_range(Some("bytes=-0"), 100).is_err());
    }

    #[test]
    fn empty_files_only_allow_a_full_request() {
        assert_eq!(requested_range(None, 0).unwrap(), (0, 0, false));
        assert!(requested_range(Some("bytes=0-"), 0).is_err());
    }
}
