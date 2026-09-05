// SPDX-License-Identifier: MIT
//! Каталог весов whisper.cpp: где лежат, откуда качать, как проверить.
//!
//! Загрузка идёт только по HTTPS и только на известный SHA-256: хеши зафиксированы в каталоге
//! из LFS-указателей репозитория `ggerganov/whisper.cpp`, поэтому подменённый или недокачанный
//! файл до диска не доезжает. Скачивание идёт в `<имя>.bin.part` и переименовывается только
//! после успешной проверки, а прерванная загрузка продолжается запросом `Range`.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::config::Config;

/// База, откуда берутся веса; вынесена отдельно, чтобы её было видно в документации и в тестах.
pub const HF_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// Размер куска при чтении сети и при подсчёте хеша.
const CHUNK: usize = 64 * 1024;

/// Описание одного набора весов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    /// Имя для CLI и конфига: `small`, `large-v3-turbo-q5_0`.
    pub name: &'static str,
    pub file_name: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
    /// Ожидаемый SHA-256 файла, нижний регистр, 64 hex-символа.
    pub sha256: &'static str,
}

/// Каталог моделей. Размеры и хеши взяты из LFS-указателей Hugging Face 05.09.2026.
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
    #[error("неизвестная модель {name}. Доступны: {known}")]
    UnknownModel { name: String, known: String },
    #[error("модель {model} не установлена. Скачайте: molva models pull {model}")]
    NotInstalled { model: String, path: PathBuf },
    #[error("не удалось определить каталог данных пользователя")]
    NoHome,
    #[error("сеть недоступна или сервер ответил ошибкой ({url}): {reason}")]
    Http { url: String, reason: String },
    #[error("ошибка работы с файлом {path}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("контрольная сумма {model} не совпала: ожидалось {expected}, получено {actual}; файл удалён")]
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

/// Состояние одной модели на диске — то, что печатает `molva models list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelStatus {
    #[serde(flatten)]
    pub info: ModelInfo,
    pub installed: bool,
    /// Фактический размер файла; 0, если модели нет.
    pub size_on_disk: u64,
    pub path: PathBuf,
}

/// Найти модель по имени.
pub fn find(name: &str) -> Result<&'static ModelInfo, ModelError> {
    CATALOG
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| ModelError::UnknownModel {
            name: name.to_string(),
            known: known_names(),
        })
}

/// Список имён каталога через запятую — для подсказок в ошибках.
pub fn known_names() -> String {
    CATALOG
        .iter()
        .map(|m| m.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Каталог, где лежат веса: `stt.model_path`, если задан, иначе каталог данных пользователя.
pub fn models_dir(cfg: &Config) -> Result<PathBuf, ModelError> {
    let configured = cfg.stt.model_path.trim();
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    directories::ProjectDirs::from("", "", "molva")
        .map(|dirs| dirs.data_dir().join("models"))
        .ok_or(ModelError::NoHome)
}

/// Путь к файлу конкретной модели, независимо от того, скачана она или нет.
pub fn model_path(cfg: &Config, name: &str) -> Result<PathBuf, ModelError> {
    let info = find(name)?;
    Ok(models_dir(cfg)?.join(info.file_name))
}

/// Путь к уже установленной модели; иначе — ошибка с командой для скачивания (O-03).
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

/// Что из каталога есть на диске.
pub fn list(dir: &Path) -> Vec<ModelStatus> {
    CATALOG
        .iter()
        .map(|info| {
            let path = dir.join(info.file_name);
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

/// Удалить установленную модель; возвращает путь удалённого файла.
pub fn remove(name: &str, dir: &Path) -> Result<PathBuf, ModelError> {
    let info = find(name)?;
    let path = dir.join(info.file_name);
    if !path.exists() {
        return Err(ModelError::NotInstalled {
            model: name.to_string(),
            path,
        });
    }
    std::fs::remove_file(&path).map_err(|e| io_err(&path, &e))?;
    Ok(path)
}

/// SHA-256 файла в нижнем регистре.
pub fn sha256_file(path: &Path) -> Result<String, ModelError> {
    let mut file = std::fs::File::open(path).map_err(|e| io_err(path, &e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buf).map_err(|e| io_err(path, &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Совпадает ли файл с ожидаемой суммой. Отсутствующий файл — это `false`, а не ошибка.
pub fn verify(path: &Path, expected: &str) -> Result<bool, ModelError> {
    if !path.is_file() {
        return Ok(false);
    }
    Ok(sha256_file(path)?.eq_ignore_ascii_case(expected.trim()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, b| {
        // Запись в String не может не удаться, но unwrap здесь всё равно ни к чему.
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Скачать модель из каталога в `dir`.
///
/// Если файл уже на месте и хеш совпадает, сеть не трогается вовсе (A-09): повторный `pull`
/// стоит одного чтения диска. `progress` вызывается по мере загрузки: (скачано, всего).
pub fn pull(
    name: &str,
    dir: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, ModelError> {
    let info = find(name)?;
    download_verified(info.url, dir, info.file_name, info.sha256, name, progress)
}

/// Загрузка произвольного файла с проверкой SHA-256 — ядро `pull`, вынесенное ради тестов.
pub fn download_verified(
    url: &str,
    dir: &Path,
    file_name: &str,
    sha256: &str,
    label: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, ModelError> {
    let target = dir.join(file_name);
    if verify(&target, sha256)? {
        let size = std::fs::metadata(&target).map_or(0, |m| m.len());
        progress(size, size);
        return Ok(target);
    }
    if target.exists() {
        // Файл есть, но битый: качаем заново, чтобы не выдавать мусор за модель.
        std::fs::remove_file(&target).map_err(|e| io_err(&target, &e))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, &e))?;

    let part = dir.join(format!("{file_name}.part"));
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

    // 206 — сервер продолжил с нужного места; 200 — отдал файл целиком, докачка не удалась.
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

    let mut buf = vec![0u8; CHUNK];
    loop {
        let read = response.read(&mut buf).map_err(|e| ModelError::Http {
            url: url.to_string(),
            reason: e.to_string(),
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])
            .map_err(|e| io_err(&part, &e))?;
        downloaded += read as u64;
        progress(downloaded, total.max(downloaded));
    }
    file.flush().map_err(|e| io_err(&part, &e))?;
    drop(file);

    std::fs::rename(&part, &target).map_err(|e| io_err(&part, &e))?;

    let actual = sha256_file(&target)?;
    if !actual.eq_ignore_ascii_case(sha256.trim()) {
        // Испорченный файл не должен пережить неудачную загрузку: иначе следующий запуск
        // попытается скормить его whisper и упадёт непонятной ошибкой.
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

    /// Локальный HTTP-сервер на эфемерном порту: отдаёт заданные байты, понимает `Range`.
    ///
    /// Возвращает URL и счётчик обслуженных запросов — по нему тест видит, ходили ли в сеть.
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
        assert_eq!(names.len(), before, "имена моделей повторяются");
        for m in CATALOG {
            assert_eq!(m.sha256.len(), 64, "{}: {}", m.name, m.sha256);
            assert!(
                m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: не hex",
                m.name
            );
            assert!(
                m.size_bytes > 1_000_000,
                "{}: подозрительный размер",
                m.name
            );
            assert!(m.url.starts_with("https://"), "{}: не HTTPS", m.name);
            assert!(
                m.url.ends_with(m.file_name),
                "{}: url и файл разошлись",
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

    #[test]
    fn missing_model_error_shows_the_pull_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.stt.model_path = dir.path().display().to_string();
        let err = installed_path(&cfg, "small").unwrap_err();
        assert!(err.to_string().contains("molva models pull small"), "{err}");
    }

    #[test]
    fn download_writes_file_when_checksum_matches() {
        let body = vec![7u8; 200_000];
        let sha = sha_of(&body);
        let (url, served) = serve(body.clone(), 1);
        let dir = tempfile::tempdir().unwrap();

        let mut seen: Vec<(u64, u64)> = Vec::new();
        let path = download_verified(&url, dir.path(), "m.bin", &sha, "m", &mut |d, t| {
            seen.push((d, t));
        })
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(served.load(Ordering::SeqCst), 1);
        assert_eq!(seen.last().unwrap().0, body.len() as u64);
        assert!(
            seen.len() > 1,
            "прогресс должен вызываться по ходу загрузки"
        );
        assert!(!dir.path().join("m.bin.part").exists(), ".part не убран");
    }

    #[test]
    fn checksum_mismatch_deletes_the_file() {
        let body = vec![1u8; 4096];
        let wrong = "0".repeat(64);
        let (url, _) = serve(body, 1);
        let dir = tempfile::tempdir().unwrap();

        let err = download_verified(&url, dir.path(), "m.bin", &wrong, "m", &mut no_progress())
            .unwrap_err();
        assert!(matches!(err, ModelError::ChecksumMismatch { .. }), "{err}");
        assert!(
            !dir.path().join("m.bin").exists(),
            "битый файл остался на диске"
        );
        assert!(!dir.path().join("m.bin.part").exists());
    }

    #[test]
    fn second_pull_does_not_touch_the_network() {
        let body = vec![3u8; 10_000];
        let sha = sha_of(&body);
        // Сервер готов обслужить только один запрос: второй `pull` обязан обойтись без сети.
        let (url, served) = serve(body, 1);
        let dir = tempfile::tempdir().unwrap();

        download_verified(&url, dir.path(), "m.bin", &sha, "m", &mut no_progress()).unwrap();
        download_verified(&url, dir.path(), "m.bin", &sha, "m", &mut no_progress()).unwrap();

        assert_eq!(served.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn interrupted_download_resumes_from_part_file() {
        let body: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        let sha = sha_of(&body);
        let (url, _) = serve(body.clone(), 1);
        let dir = tempfile::tempdir().unwrap();
        // Половина файла уже скачана прошлым, прерванным запуском.
        let half = body.len() / 2;
        std::fs::write(dir.path().join("m.bin.part"), &body[..half]).unwrap();

        let mut first_report = None;
        let path = download_verified(&url, dir.path(), "m.bin", &sha, "m", &mut |d, t| {
            first_report.get_or_insert((d, t));
        })
        .unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(
            first_report,
            Some((half as u64, body.len() as u64)),
            "докачка должна стартовать с уже скачанного места"
        );
    }

    #[test]
    fn corrupted_existing_file_is_redownloaded() {
        let body = vec![9u8; 8_192];
        let sha = sha_of(&body);
        let (url, _) = serve(body.clone(), 1);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.bin"), "мусор").unwrap();

        let path =
            download_verified(&url, dir.path(), "m.bin", &sha, "m", &mut no_progress()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
    }

    #[test]
    fn server_error_is_reported_with_url() {
        let dir = tempfile::tempdir().unwrap();
        // Порт, который никто не слушает: соединение не установится.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}/model.bin");

        let err = download_verified(
            &url,
            dir.path(),
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
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-tiny.bin"), vec![0u8; 42]).unwrap();
        let statuses = list(dir.path());
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ggml-tiny.bin");
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(remove("tiny", dir.path()).unwrap(), path);
        assert!(!path.exists());
        assert!(matches!(
            remove("tiny", dir.path()).unwrap_err(),
            ModelError::NotInstalled { .. }
        ));
    }

    #[test]
    fn verify_compares_hash_and_tolerates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.bin");
        std::fs::write(&path, b"molva").unwrap();
        let sha = sha_of(b"molva");
        assert!(verify(&path, &sha).unwrap());
        assert!(verify(&path, &sha.to_uppercase()).unwrap());
        assert!(!verify(&path, &"0".repeat(64)).unwrap());
        assert!(!verify(&dir.path().join("absent.bin"), &sha).unwrap());
    }

    #[test]
    fn sha256_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
