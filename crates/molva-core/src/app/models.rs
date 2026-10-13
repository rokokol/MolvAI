// SPDX-License-Identifier: MIT
//! Catalog of whisper.cpp model weights: where they live, where to download them, how to check
//! them
//!
//! The weights download from Hugging Face over HTTPS or from the mirror in `stt.models_mirror`,
//! and only against a known SHA-256. The hashes are fixed in the catalog from the LFS pointers
//! of the `ggerganov/whisper.cpp` repository. So a replaced or incomplete file never reaches
//! the disk, even if the mirror serves it over `http://`. The download goes to
//! `<name>.bin.part` and is renamed only after a successful check. An interrupted download
//! resumes with a `Range` request

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::Config;

/// Base address of the weights; a separate constant, so it is visible in the docs and in tests
pub const HF_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// Size of a chunk when reading the network and when computing the hash
const CHUNK: usize = 64 * 1024;

/// Description of one set of weights
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    /// Name for the CLI and the config: `small`, `large-v3-turbo-q5_0`
    pub name: &'static str,
    pub file_name: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
    /// Expected SHA-256 of the file, lower case, 64 hex characters
    pub sha256: &'static str,
}

/// Catalog of models. The sizes and hashes come from the Hugging Face LFS pointers of 2026-09-05
pub const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        name: "tiny",
        file_name: "ggml-tiny.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    },
    ModelInfo {
        name: "base",
        file_name: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    },
    ModelInfo {
        name: "small",
        file_name: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    },
    ModelInfo {
        name: "small-q5_1",
        file_name: "ggml-small-q5_1.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",
        size_bytes: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
    },
    ModelInfo {
        name: "medium",
        file_name: "ggml-medium.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        size_bytes: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    },
    ModelInfo {
        name: "large-v3-turbo",
        file_name: "ggml-large-v3-turbo.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
    },
    ModelInfo {
        name: "large-v3-turbo-q5_0",
        file_name: "ggml-large-v3-turbo-q5_0.bin",
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    },
    ModelInfo {
        name: "large-v3-turbo-q8_0",
        file_name: "ggml-large-v3-turbo-q8_0.bin",
        url:
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin",
        size_bytes: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
    },
    ModelInfo {
        name: "large-v3",
        file_name: "ggml-large-v3.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        size_bytes: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
    },
];

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("unknown model {name}. Available: {known}")]
    UnknownModel { name: String, known: String },
    #[error("model {model} is not installed. Download it with: molva models pull {model}")]
    NotInstalled { model: String, path: PathBuf },
    #[error("cannot find the user data directory")]
    NoHome,
    #[error("network not available or the server answered with an error ({url}): {reason}")]
    Http { url: String, reason: String },
    #[error("file error {path}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("checksum of {model} does not match: expected {expected}, got {actual}; file removed")]
    ChecksumMismatch {
        model: String,
        expected: String,
        actual: String,
    },
}

fn io_err(path: &Path, e: &std::io::Error) -> ModelError {
    ModelError::Io {
        path: path.to_path_buf(),
        reason: e.to_string(),
    }
}

/// State of one model on disk, as `molva models list` prints it
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelStatus {
    #[serde(flatten)]
    pub info: ModelInfo,
    pub installed: bool,
    /// Actual size of the file; 0 if the model is not there
    pub size_on_disk: u64,
    pub path: PathBuf,
}

/// Find a model by name
pub fn find(name: &str) -> Result<&'static ModelInfo, ModelError> {
    CATALOG
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| ModelError::UnknownModel {
            name: name.to_string(),
            known: known_names(),
        })
}

/// List of the catalog names, separated by commas, for hints in errors
pub fn known_names() -> String {
    CATALOG
        .iter()
        .map(|m| m.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Directory with the weights: `stt.model_path` if set, otherwise the user data directory
pub fn models_dir(cfg: &Config) -> Result<PathBuf, ModelError> {
    let configured = cfg.stt.model_path.trim();
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    directories::ProjectDirs::from("", "", "molva")
        .map(|dirs| dirs.data_dir().join("models"))
        .ok_or(ModelError::NoHome)
}

/// Path to the file of a given model, whether it is downloaded or not
pub fn model_path(cfg: &Config, name: &str) -> Result<PathBuf, ModelError> {
    let info = find(name)?;
    Ok(models_dir(cfg)?.join(info.file_name))
}

/// Path to an installed model; otherwise an error with the download command (O-03)
pub fn installed_path(cfg: &Config, name: &str) -> Result<PathBuf, ModelError> {
    let path = model_path(cfg, name)?;
    if path.is_file() {
        return Ok(path);
    }
    Err(ModelError::NotInstalled {
        model: name.to_string(),
        path,
    })
}

/// Which models of the catalog are on disk
pub fn list(directory: &Path) -> Vec<ModelStatus> {
    CATALOG
        .iter()
        .map(|info| {
            let path = directory.join(info.file_name);
            let size_on_disk = std::fs::metadata(&path).map_or(0, |m| m.len());
            ModelStatus {
                info: *info,
                installed: size_on_disk > 0,
                size_on_disk,
                path,
            }
        })
        .collect()
}

/// Remove an installed model; returns the path of the removed file
pub fn remove(name: &str, directory: &Path) -> Result<PathBuf, ModelError> {
    let info = find(name)?;
    let path = directory.join(info.file_name);
    if !path.exists() {
        return Err(ModelError::NotInstalled {
            model: name.to_string(),
            path,
        });
    }
    std::fs::remove_file(&path).map_err(|e| io_err(&path, &e))?;
    Ok(path)
}

/// SHA-256 of the file, in lower case
pub fn sha256_file(path: &Path) -> Result<String, ModelError> {
    let mut file = std::fs::File::open(path).map_err(|e| io_err(path, &e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buffer).map_err(|e| io_err(path, &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Whether the file matches the expected sum. A missing file gives `false`, not an error
pub fn verify(path: &Path, expected: &str) -> Result<bool, ModelError> {
    if !path.is_file() {
        return Ok(false);
    }
    Ok(sha256_file(path)?.eq_ignore_ascii_case(expected.trim()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, b| {
        // A write to a String cannot fail, but an unwrap is not needed here anyway
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Download a model from the catalog into `directory`
///
/// If the file is already in place and the hash matches, the network is not touched at all
/// (A-09): a repeated `pull` costs one disk read. `progress` is called during the download
/// with (downloaded, total)
pub fn pull(
    name: &str,
    directory: &Path,
    mirror: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, ModelError> {
    let info = find(name)?;
    download_verified(
        &download_url(info, mirror),
        directory,
        info.file_name,
        info.sha256,
        name,
        progress,
    )
}

/// Address of the model file: the mirror from `stt.models_mirror` or Hugging Face from the catalog
pub fn download_url(info: &ModelInfo, mirror: &str) -> String {
    let mirror = mirror.trim();
    if mirror.is_empty() {
        return info.url.to_string();
    }
    format!("{}/{}", mirror.trim_end_matches('/'), info.file_name)
}

/// How long a `.part` file may stay unchanged while the download still counts as active
///
/// The fragment of an interrupted download stays until the next `pull`. Without this window it
/// would look like an endless download stuck at one percent
pub const ACTIVE_DOWNLOAD_WINDOW: Duration = Duration::from_secs(15);

/// Download of a model that this process or another process runs now
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DownloadProgress {
    pub model: String,
    pub downloaded: u64,
    pub total: u64,
}

/// File where `download_verified` writes the bytes before the checksum check
pub fn partial_path(directory: &Path, file_name: &str) -> PathBuf {
    directory.join(format!("{file_name}.part"))
}

/// Whether a download of the model `name` into the settings directory is active, judging by its
/// `.part` file
///
/// The downloading process does not matter: the daemon with `stt.auto_pull`, `molva models pull`
/// or the GUI
pub fn active_download(cfg: &Config, name: &str, now: SystemTime) -> Option<DownloadProgress> {
    let info = find(name).ok()?;
    let metadata = std::fs::metadata(partial_path(&models_dir(cfg).ok()?, info.file_name)).ok()?;
    // A clock that went backwards gives an error: such a file counts as fresh, not abandoned
    let idle = now
        .duration_since(metadata.modified().ok()?)
        .unwrap_or(Duration::ZERO);
    if idle > ACTIVE_DOWNLOAD_WINDOW {
        return None;
    }
    Some(DownloadProgress {
        model: info.name.to_string(),
        downloaded: metadata.len(),
        total: info.size_bytes.max(metadata.len()),
    })
}

/// Download of any file with a SHA-256 check: the core of `pull`, split out for tests
pub fn download_verified(
    url: &str,
    directory: &Path,
    file_name: &str,
    sha256: &str,
    label: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, ModelError> {
    let target = directory.join(file_name);
    if verify(&target, sha256)? {
        let size = std::fs::metadata(&target).map_or(0, |m| m.len());
        progress(size, size);
        return Ok(target);
    }
    if target.exists() {
        // The file exists but is broken: download again, to avoid passing garbage off as a model
        std::fs::remove_file(&target).map_err(|e| io_err(&target, &e))?;
    }
    std::fs::create_dir_all(directory).map_err(|e| io_err(directory, &e))?;

    let part = partial_path(directory, file_name);
    let already = std::fs::metadata(&part).map_or(0, |m| m.len());

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("molva/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ModelError::Http {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
    let mut request = client.get(url);
    if already > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={already}-"));
    }
    let mut response = request.send().map_err(|e| ModelError::Http {
        url: url.to_string(),
        reason: e.to_string(),
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ModelError::Http {
            url: url.to_string(),
            reason: format!("HTTP {status}"),
        });
    }

    // 206: the server continued from the right place; 200: it sent the whole file, so the
    // resume failed
    let resuming = already > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
    let remaining = response.content_length().unwrap_or(0);
    let total = if resuming {
        already + remaining
    } else {
        remaining
    };

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!resuming)
        .open(&part)
        .map_err(|e| io_err(&part, &e))?;
    let mut downloaded = if resuming {
        file.seek(SeekFrom::End(0)).map_err(|e| io_err(&part, &e))?;
        already
    } else {
        0
    };
    progress(downloaded, total);

    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = response.read(&mut buffer).map_err(|e| ModelError::Http {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| io_err(&part, &e))?;
        downloaded += read as u64;
        progress(downloaded, total.max(downloaded));
    }
    file.flush().map_err(|e| io_err(&part, &e))?;
    drop(file);

    std::fs::rename(&part, &target).map_err(|e| io_err(&part, &e))?;

    let actual = sha256_file(&target)?;
    if !actual.eq_ignore_ascii_case(sha256.trim()) {
        // A corrupted file must not survive a failed download. Otherwise the next start would
        // feed it to whisper and fail with an unclear error
        let _ = std::fs::remove_file(&target);
        return Err(ModelError::ChecksumMismatch {
            model: label.to_string(),
            expected: sha256.to_string(),
            actual,
        });
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn sha_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex(&hasher.finalize())
    }

    /// Local HTTP server on an ephemeral port: serves the given bytes and understands `Range`
    ///
    /// Returns the URL and a counter of served requests, with which a test sees whether the
    /// network was used
    fn serve(body: Vec<u8>, requests: usize) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let served = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&served);
        std::thread::spawn(move || {
            for _ in 0..requests {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream);
                let mut range_from = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("range: bytes=")
                        .map(str::to_string)
                    {
                        range_from = value.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                }
                let chunk = &body[range_from.min(body.len())..];
                let head = if range_from > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        chunk.len(),
                        range_from,
                        body.len().saturating_sub(1),
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        chunk.len()
                    )
                };
                let mut stream = reader.into_inner();
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(chunk);
                let _ = stream.flush();
                counter.fetch_add(1, Ordering::SeqCst);
            }
        });
        (format!("http://{addr}/model.bin"), served)
    }

    fn no_progress() -> impl FnMut(u64, u64) {
        |_, _| {}
    }

    #[test]
    fn catalog_names_are_unique_and_hashes_look_like_sha256() {
        let mut names: Vec<_> = CATALOG.iter().map(|m| m.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "model names repeat");
        for m in CATALOG {
            assert_eq!(m.sha256.len(), 64, "{}: {}", m.name, m.sha256);
            assert!(
                m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: not hex",
                m.name
            );
            assert!(m.size_bytes > 1_000_000, "{}: suspicious size", m.name);
            assert!(m.url.starts_with("https://"), "{}: not HTTPS", m.name);
            assert!(
                m.url.ends_with(m.file_name),
                "{}: the url and the file differ",
                m.name
            );
        }
    }

    #[test]
    fn unknown_model_lists_the_known_ones() {
        let err = find("huge").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("huge"), "{text}");
        assert!(text.contains("large-v3-turbo"), "{text}");
    }

    #[test]
    fn models_dir_follows_configured_path() {
        let mut cfg = Config::default();
        cfg.stt.model_path = "/opt/weights".into();
        assert_eq!(models_dir(&cfg).unwrap(), PathBuf::from("/opt/weights"));
        assert_eq!(
            model_path(&cfg, "small").unwrap(),
            PathBuf::from("/opt/weights/ggml-small.bin")
        );
    }

    fn config_in(directory: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.stt.model_path = directory.display().to_string();
        cfg
    }

    #[test]
    fn a_fresh_part_file_is_an_active_download() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = config_in(directory.path());
        let info = find("small").unwrap();
        std::fs::write(
            partial_path(directory.path(), info.file_name),
            vec![0u8; 4096],
        )
        .unwrap();
        let progress = active_download(&cfg, "small", SystemTime::now()).unwrap();
        assert_eq!(
            progress,
            DownloadProgress {
                model: "small".into(),
                downloaded: 4096,
                total: info.size_bytes,
            }
        );
    }

    #[test]
    fn without_a_part_file_nothing_is_downloading() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = config_in(directory.path());
        assert_eq!(active_download(&cfg, "small", SystemTime::now()), None);
    }

    #[test]
    fn a_part_file_left_by_an_interrupted_download_is_not_active() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = config_in(directory.path());
        let part = partial_path(directory.path(), find("small").unwrap().file_name);
        std::fs::write(&part, vec![0u8; 4096]).unwrap();
        let later = SystemTime::now() + ACTIVE_DOWNLOAD_WINDOW + Duration::from_secs(1);
        assert_eq!(active_download(&cfg, "small", later), None);
    }

    #[test]
    fn an_unknown_model_has_no_download() {
        let directory = tempfile::tempdir().unwrap();
        let cfg = config_in(directory.path());
        // Every model of the catalog can download: an unknown name must not take another download
        for info in CATALOG {
            std::fs::write(partial_path(directory.path(), info.file_name), b"x").unwrap();
        }
        assert_eq!(active_download(&cfg, "huge", SystemTime::now()), None);
    }

    #[test]
    fn the_downloader_writes_where_active_download_looks() {
        let body = vec![5u8; 300_000];
        let sha = sha_of(&body);
        let (url, _) = serve(body, 1);
        let directory = tempfile::tempdir().unwrap();
        let part = partial_path(directory.path(), "m.bin");
        let mut seen_mid_download = false;
        download_verified(
            &url,
            directory.path(),
            "m.bin",
            &sha,
            "m",
            &mut |done, total| {
                if done > 0 && done < total {
                    let on_disk = std::fs::metadata(&part).map_or(0, |m| m.len());
                    assert!(
                        on_disk > 0,
                        "{} is empty in the middle of the download",
                        part.display()
                    );
                    seen_mid_download = true;
                }
            },
        )
        .unwrap();
        assert!(
            seen_mid_download,
            "progress did not report any middle of the download"
        );
        assert!(
            !part.exists(),
            "{} remained after the download",
            part.display()
        );
    }

    #[test]
    fn without_a_mirror_the_catalog_url_is_used() {
        let info = find("small").unwrap();
        assert_eq!(download_url(info, ""), info.url);
        assert_eq!(download_url(info, "  "), info.url);
    }

    #[test]
    fn a_mirror_is_joined_with_the_file_name_by_one_slash() {
        let info = find("small").unwrap();
        for mirror in [
            "https://hf-mirror.com/whisper",
            "https://hf-mirror.com/whisper/",
        ] {
            assert_eq!(
                download_url(info, mirror),
                "https://hf-mirror.com/whisper/ggml-small.bin"
            );
        }
    }

    #[test]
    fn pull_downloads_from_the_mirror() {
        // Port 9 is closed: the request fails at once, and the error names the address used
        let directory = tempfile::tempdir().unwrap();
        let err = pull(
            "tiny",
            directory.path(),
            "http://127.0.0.1:9/weights",
            &mut no_progress(),
        )
        .unwrap_err();
        match err {
            ModelError::Http { url, .. } => {
                assert_eq!(url, "http://127.0.0.1:9/weights/ggml-tiny.bin");
            }
            other => panic!("expected a network error, got {other}"),
        }
    }

    #[test]
    fn missing_model_error_shows_the_pull_command() {
        let directory = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.stt.model_path = directory.path().display().to_string();
        let err = installed_path(&cfg, "small").unwrap_err();
        assert!(err.to_string().contains("molva models pull small"), "{err}");
    }

    #[test]
    fn download_writes_file_when_checksum_matches() {
        let body = vec![7u8; 200_000];
        let sha = sha_of(&body);
        let (url, served) = serve(body.clone(), 1);
        let directory = tempfile::tempdir().unwrap();

        let mut seen: Vec<(u64, u64)> = Vec::new();
        let path = download_verified(&url, directory.path(), "m.bin", &sha, "m", &mut |d, t| {
            seen.push((d, t));
        })
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(served.load(Ordering::SeqCst), 1);
        assert_eq!(seen.last().unwrap().0, body.len() as u64);
        assert!(
            seen.len() > 1,
            "progress must be called during the download"
        );
        assert!(
            !directory.path().join("m.bin.part").exists(),
            ".part was not removed"
        );
    }

    #[test]
    fn checksum_mismatch_deletes_the_file() {
        let body = vec![1u8; 4096];
        let wrong = "0".repeat(64);
        let (url, _) = serve(body, 1);
        let directory = tempfile::tempdir().unwrap();

        let err = download_verified(
            &url,
            directory.path(),
            "m.bin",
            &wrong,
            "m",
            &mut no_progress(),
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::ChecksumMismatch { .. }), "{err}");
        assert!(
            !directory.path().join("m.bin").exists(),
            "the broken file remained on disk"
        );
        assert!(!directory.path().join("m.bin.part").exists());
    }

    #[test]
    fn second_pull_does_not_touch_the_network() {
        let body = vec![3u8; 10_000];
        let sha = sha_of(&body);
        // The server serves only one request: the second `pull` must work without the network
        let (url, served) = serve(body, 1);
        let directory = tempfile::tempdir().unwrap();

        download_verified(
            &url,
            directory.path(),
            "m.bin",
            &sha,
            "m",
            &mut no_progress(),
        )
        .unwrap();
        download_verified(
            &url,
            directory.path(),
            "m.bin",
            &sha,
            "m",
            &mut no_progress(),
        )
        .unwrap();

        assert_eq!(served.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn interrupted_download_resumes_from_part_file() {
        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let sha = sha_of(&body);
        let (url, _) = serve(body.clone(), 1);
        let directory = tempfile::tempdir().unwrap();
        // Half of the file is already downloaded by the previous, interrupted run
        let half = body.len() / 2;
        std::fs::write(directory.path().join("m.bin.part"), &body[..half]).unwrap();

        let mut first_report = None;
        let path = download_verified(&url, directory.path(), "m.bin", &sha, "m", &mut |d, t| {
            first_report.get_or_insert((d, t));
        })
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(
            first_report,
            Some((half as u64, body.len() as u64)),
            "the resume must start from the place already downloaded"
        );
    }

    #[test]
    fn corrupted_existing_file_is_redownloaded() {
        let body = vec![9u8; 8_192];
        let sha = sha_of(&body);
        let (url, _) = serve(body.clone(), 1);
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("m.bin"), "garbage").unwrap();

        let path = download_verified(
            &url,
            directory.path(),
            "m.bin",
            &sha,
            "m",
            &mut no_progress(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
    }

    #[test]
    fn server_error_is_reported_with_url() {
        let directory = tempfile::tempdir().unwrap();
        // A port that nobody listens on: the connection will not open
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}/model.bin");

        let err = download_verified(
            &url,
            directory.path(),
            "m.bin",
            &"a".repeat(64),
            "m",
            &mut no_progress(),
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::Http { .. }), "{err}");
        assert!(err.to_string().contains(&addr.to_string()), "{err}");
    }

    #[test]
    fn list_reports_installed_and_size() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("ggml-tiny.bin"), vec![0u8; 42]).unwrap();
        let statuses = list(directory.path());
        assert_eq!(statuses.len(), CATALOG.len());
        let tiny = statuses.iter().find(|s| s.info.name == "tiny").unwrap();
        assert!(tiny.installed);
        assert_eq!(tiny.size_on_disk, 42);
        let small = statuses.iter().find(|s| s.info.name == "small").unwrap();
        assert!(!small.installed);
        assert_eq!(small.size_on_disk, 0);
    }

    #[test]
    fn remove_deletes_installed_model_and_reports_missing_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ggml-tiny.bin");
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(remove("tiny", directory.path()).unwrap(), path);
        assert!(!path.exists());
        assert!(matches!(
            remove("tiny", directory.path()).unwrap_err(),
            ModelError::NotInstalled { .. }
        ));
    }

    #[test]
    fn verify_compares_hash_and_tolerates_missing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("f.bin");
        std::fs::write(&path, b"molva").unwrap();
        let sha = sha_of(b"molva");
        assert!(verify(&path, &sha).unwrap());
        assert!(verify(&path, &sha.to_uppercase()).unwrap());
        assert!(!verify(&path, &"0".repeat(64)).unwrap());
        assert!(!verify(&directory.path().join("absent.bin"), &sha).unwrap());
    }

    #[test]
    fn sha256_matches_known_vector() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
