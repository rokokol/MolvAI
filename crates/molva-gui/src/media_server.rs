// SPDX-License-Identifier: MIT
//! A loopback HTTP server that plays one file at a time
//!
//! WebKitGTK hands a custom `asset://` URL to the media pipeline, and the pipeline refuses it:
//! the media element reports `MEDIA_ERR_SRC_NOT_SUPPORTED` even for a plain WAV. A file that a
//! media element can stream needs an `http://` origin, so this server answers exactly that, on a
//! loopback port with a random token.
//!
//! The reach is deliberately narrow: a request must carry the token, the path is an identifier
//! of a file that `prepare_media` was asked for in this run, and anything else is a 404. It
//! answers range requests, so a player seeks inside a long recording instead of reading it whole
//! into the window's memory.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::lock;

/// How many bytes one response carries at most when a client asks for everything
const CHUNK: usize = 64 * 1024;

/// The MIME type of a file, from its extension; a player needs a truthful one to pick a pipeline
pub fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("avi") => "video/x-msvideo",
        Some("ogv") => "video/ogg",
        Some("mpg" | "mpeg") => "video/mpeg",
        Some("wmv") => "video/x-ms-wmv",
        Some("flv") => "video/x-flv",
        Some("3gp") => "video/3gpp",
        Some("ts") => "video/mp2t",
        Some("wav" | "wave") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("m4a" | "m4b" | "aac") => "audio/mp4",
        _ => "application/octet-stream",
    }
}

/// The byte range a client asked for, in the inclusive form of the HTTP header
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: u64,
    /// Inclusive
    pub end: u64,
}

/// Parse one `Range` header value against the size of the file
///
/// Only a single byte range is understood, which is what a media element asks for. A malformed
/// or unsatisfiable range gives `None`, and the caller answers with the whole file: a player
/// that asked wrongly still gets its bytes rather than a stall.
pub fn parse_range(header: &str, size: u64) -> Option<Range> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let spec = spec.split(',').next()?.trim();
    let (start, end) = spec.split_once('-')?;
    if size == 0 {
        return None;
    }
    let (start, end) = if start.is_empty() {
        // A suffix range: the last N bytes
        let suffix: u64 = end.trim().parse().ok()?;
        if suffix == 0 {
            return None;
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = start.trim().parse().ok()?;
        let end = if end.trim().is_empty() {
            size.saturating_sub(1)
        } else {
            end.trim().parse::<u64>().ok()?.min(size.saturating_sub(1))
        };
        (start, end)
    };
    (start <= end && start < size).then_some(Range { start, end })
}

/// The URL path of one file: `/media/<token>/<id>`
fn request_target(token: &str, id: u64) -> String {
    format!("/media/{token}/{id}")
}

/// The file identifier of a request path, when the token matches
fn parse_target(target: &str, token: &str) -> Option<u64> {
    let path = target.split('?').next().unwrap_or(target);
    let rest = path.strip_prefix("/media/")?;
    let (found_token, id) = rest.split_once('/')?;
    if found_token != token {
        return None;
    }
    id.parse().ok()
}

/// Running media server: a port, a token and the files the window may play
#[derive(Debug)]
pub struct MediaServer {
    pub port: u16,
    token: String,
    files: Arc<Mutex<HashMap<u64, PathBuf>>>,
    next_id: AtomicU64,
}

impl MediaServer {
    /// Bind a loopback port and answer requests on a thread of its own
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let port = listener.local_addr()?.port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let files: Arc<Mutex<HashMap<u64, PathBuf>>> = Arc::new(Mutex::new(HashMap::new()));
        let shared = Arc::clone(&files);
        let served_token = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let files = Arc::clone(&shared);
                let token = served_token.clone();
                // One short-lived thread per request: a player opens a few of them
                std::thread::spawn(move || {
                    let _ = serve(stream, &token, &files);
                });
            }
        });
        Ok(Self {
            port,
            token,
            files,
            next_id: AtomicU64::new(1),
        })
    }

    /// The URL that plays this file, and the identifier it keeps afterwards
    pub fn allow(&self, path: &Path) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.files).insert(id, path.to_path_buf());
        format!(
            "http://127.0.0.1:{}{}",
            self.port,
            request_target(&self.token, id)
        )
    }
}

/// Read the request line and the headers, then answer
fn serve(
    mut stream: TcpStream,
    token: &str,
    files: &Mutex<HashMap<u64, PathBuf>>,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut range_header = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range_header = Some(value.trim().to_string());
            }
        }
    }
    if method != "GET" && method != "HEAD" {
        return respond_status(&mut stream, 405, "method not allowed");
    }
    let Some(id) = parse_target(&target, token) else {
        return respond_status(&mut stream, 404, "not found");
    };
    let Some(path) = lock(files).get(&id).cloned() else {
        return respond_status(&mut stream, 404, "not found");
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return respond_status(&mut stream, 404, "not found");
    };
    if !metadata.is_file() {
        return respond_status(&mut stream, 404, "not found");
    }
    let size = metadata.len();
    let range = range_header
        .as_deref()
        .and_then(|value| parse_range(value, size));
    let (status, start, end) = match range {
        Some(Range { start, end }) => (206, start, end),
        None => (200, 0, size.saturating_sub(1)),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\n\
         Content-Type: {}\r\n\
         Content-Length: {length}\r\n\
         Accept-Ranges: bytes\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Expose-Headers: content-range, accept-ranges\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n",
        if status == 206 {
            "Partial Content"
        } else {
            "OK"
        },
        content_type(&path)
    );
    if status == 206 {
        head.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if method == "HEAD" || size == 0 {
        return stream.flush();
    }
    let mut file = std::fs::File::open(&path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = length;
    let mut buffer = vec![0_u8; CHUNK];
    while remaining > 0 {
        let want = CHUNK.min(remaining as usize);
        let read = file.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        stream.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    stream.flush()
}

fn respond_status(stream: &mut TcpStream, status: u16, message: &str) -> std::io::Result<()> {
    let body = format!("{message}\n");
    let head = format!(
        "HTTP/1.1 {status} {message}\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn a_media_type_comes_from_the_extension() {
        assert_eq!(content_type(Path::new("a/lecture.MP4")), "video/mp4");
        assert_eq!(content_type(Path::new("a/voice.wav")), "audio/wav");
        assert_eq!(
            content_type(Path::new("a/unknown.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn a_closed_range_is_parsed() {
        assert_eq!(
            parse_range("bytes=10-19", 100),
            Some(Range { start: 10, end: 19 })
        );
    }

    #[test]
    fn an_open_range_runs_to_the_end() {
        assert_eq!(
            parse_range("bytes=90-", 100),
            Some(Range { start: 90, end: 99 })
        );
    }

    #[test]
    fn a_suffix_range_counts_back_from_the_end() {
        assert_eq!(
            parse_range("bytes=-10", 100),
            Some(Range { start: 90, end: 99 })
        );
    }

    #[test]
    fn an_end_beyond_the_file_is_clamped_instead_of_refused() {
        assert_eq!(
            parse_range("bytes=95-1000", 100),
            Some(Range { start: 95, end: 99 })
        );
    }

    #[test]
    fn a_useless_range_is_not_a_range() {
        assert_eq!(parse_range("items=0-1", 100), None);
        assert_eq!(parse_range("bytes=abc", 100), None);
        assert_eq!(parse_range("bytes=20-10", 100), None);
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn the_token_of_the_request_path_must_match() {
        assert_eq!(parse_target("/media/abc/7", "abc"), Some(7));
        assert_eq!(parse_target("/media/abc/7?x=1", "abc"), Some(7));
        assert_eq!(parse_target("/media/other/7", "abc"), None);
        assert_eq!(parse_target("/media/abc/seven", "abc"), None);
        assert_eq!(parse_target("/elsewhere/abc/7", "abc"), None);
    }

    fn request(port: u16, path: &str, range: Option<&str>) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the server listens");
        let mut head = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        if let Some(range) = range {
            head.push_str(&format!("Range: {range}\r\n"));
        }
        head.push_str("Connection: close\r\n\r\n");
        stream
            .write_all(head.as_bytes())
            .expect("the request is sent");
        let mut answer = String::new();
        stream
            .read_to_string(&mut answer)
            .expect("the answer arrives");
        answer
    }

    #[test]
    fn a_file_is_served_whole_and_a_range_is_served_in_part() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("blob.wav");
        std::fs::write(&path, b"0123456789").expect("the test file");
        let server = MediaServer::start().expect("the server starts");
        let url = server.allow(&path);
        let target = url
            .split_once("127.0.0.1:")
            .and_then(|(_, rest)| rest.split_once('/'))
            .map(|(_, rest)| format!("/{rest}"))
            .expect("the URL has a path");

        let whole = request(server.port, &target, None);
        assert!(whole.starts_with("HTTP/1.1 200 OK"), "{whole}");
        assert!(whole.contains("Content-Type: audio/wav"));
        assert!(whole.contains("Content-Length: 10"));
        assert!(whole.ends_with("0123456789"));

        let part = request(server.port, &target, Some("bytes=2-4"));
        assert!(part.starts_with("HTTP/1.1 206 Partial Content"), "{part}");
        assert!(part.contains("Content-Range: bytes 2-4/10"));
        assert!(part.ends_with("234"));
    }

    #[test]
    fn a_request_without_the_token_is_refused() {
        let server = MediaServer::start().expect("the server starts");
        let answer = request(server.port, "/media/guess/1", None);
        assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
    }

    #[test]
    fn a_file_that_was_never_allowed_is_refused_even_with_the_token() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("blob.wav");
        std::fs::write(&path, b"0123456789").expect("the test file");
        let server = MediaServer::start().expect("the server starts");
        let url = server.allow(&path);
        let target = url
            .split_once("127.0.0.1:")
            .and_then(|(_, rest)| rest.split_once('/'))
            .map(|(_, rest)| format!("/{rest}"))
            .expect("the URL has a path");
        // The token is right, the identifier is not one this run handed out
        let forged = target
            .rsplit_once('/')
            .map(|(head, _)| format!("{head}/99"));
        let answer = request(server.port, forged.as_deref().unwrap_or(&target), None);
        assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
    }

    #[test]
    fn a_head_request_answers_with_headers_only() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("blob.mp4");
        std::fs::write(&path, b"0123456789").expect("the test file");
        let server = MediaServer::start().expect("the server starts");
        let url = server.allow(&path);
        let target = url
            .split_once("127.0.0.1:")
            .and_then(|(_, rest)| rest.split_once('/'))
            .map(|(_, rest)| format!("/{rest}"))
            .expect("the URL has a path");
        let mut stream =
            TcpStream::connect(("127.0.0.1", server.port)).expect("the server listens");
        stream
            .write_all(format!("HEAD {target} HTTP/1.1\r\nConnection: close\r\n\r\n").as_bytes())
            .expect("the request is sent");
        let mut answer = String::new();
        stream
            .read_to_string(&mut answer)
            .expect("the answer arrives");
        assert!(answer.contains("HTTP/1.1 200 OK"), "{answer}");
        assert!(answer.contains("Content-Type: video/mp4"));
        assert!(!answer.contains("0123456789"));
    }
}
