// SPDX-License-Identifier: MIT
//! Phrase journal: a JSONL file, one line per phrase
//!
//! The line schema is described in `docs/journal-schema.md`, and the reference type is
//! `domain::entry::Entry`. The file is append-only. Deletion, clearing and rotation work by an
//! atomic rewrite (a temporary file next to it + `rename`), so a crash in the middle of an
//! operation leaves no stump
//!
//! No file handle is kept between writes: each write opens the file again. So the `&self`
//! methods that replace the file through `rename` do not leave the journal writing to a deleted
//! inode
//!
//! Every write takes a lock on `<journal>.lock`, which keeps its inode across a rewrite: writers
//! exclude each other, and a reader that takes the shared lock sees the file either before or
//! after a rewrite, never during it. A process that does not take the lock — an older build of
//! the daemon — is not excluded, and a network file system gives no promise beyond what its own
//! lock daemon keeps
//!
//! A line that does not parse goes to `<journal>.broken`. One holder of the exclusive lock moves
//! it, and that holder reads the file again under the lock, so two readers cannot move the same
//! line twice: the second one finds the journal clean. A reader that cannot take the exclusive
//! lock at this instant reads without it and leaves the line to the next write

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::app::journal_crypto::{self, JournalCipher};
use crate::app::secrets::{self, SecretStore};
use crate::domain::entry::{Entry, SCHEMA_VERSION};
use crate::domain::journal::{Journal, JournalError};

/// File header: the schema version and the creation time. `load_all` skips it
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    schema: u32,
    created: DateTime<Utc>,
}

fn io_err(path: &Path, err: &std::io::Error) -> JournalError {
    JournalError::Io(format!("{}: {err}", path.display()))
}

/// Lock file that belongs to `path`: a rewrite replaces the journal by `rename`, and a lock taken
/// on the replaced inode leaves the next writer free, so the lock lives on a file of its own
fn sibling_lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

/// Lock held for the length of one operation, released when the guard drops
///
/// The lock orders the processes that take it, and nothing more: a process that writes the
/// journal without it is still able to lose a rewrite. It also must not outlive the operation —
/// a guard kept in `FileJournal` between calls would deadlock the next call of the same process
struct JournalLock(File);

impl std::fmt::Debug for JournalLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("JournalLock").field(&self.0).finish()
    }
}

impl JournalLock {
    /// Writers wait for each other and for the readers
    fn exclusive(path: &Path) -> Result<Self, JournalError> {
        let Some(file) = open_lock_file(path)? else {
            return Err(JournalError::Io(format!(
                "cannot lock {}: the directory allows no lock file, and a write without the lock \
                 could lose the line another process appends",
                path.display()
            )));
        };
        // The trait is named in full: `File::lock` of the standard library arrived in Rust 1.89,
        // and this workspace keeps 1.85 as its floor
        // An operation without its lock loses a concurrent write: the caller gets the error
        // instead, and no line reaches the journal half done
        fs4::FileExt::lock(&file)
            .map_err(|e| JournalError::Io(format!("cannot lock {}: {e}", path.display())))?;
        Ok(Self(file))
    }

    /// The lock, or `None` when another process holds the journal at this instant
    ///
    /// A reader takes the exclusive lock this way, because a read must not wait for a writer, and
    /// moves a damaged line only under the exclusive lock. `exclusive = false` asks for the shared
    /// lock, which a read takes when other readers hold the file. `None` also answers a directory
    /// that allows no lock file at all: a read goes on, a write asks for the blocking lock instead
    fn try_acquire(path: &Path, exclusive: bool) -> Result<Option<Self>, JournalError> {
        let Some(file) = open_lock_file(path)? else {
            warn!(
                path = %path.display(),
                "no lock file is available, the journal is read without the lock"
            );
            return Ok(None);
        };
        let locked = if exclusive {
            fs4::FileExt::try_lock(&file)
        } else {
            fs4::FileExt::try_lock_shared(&file)
        };
        match locked {
            Ok(()) => Ok(Some(Self(file))),
            Err(fs4::TryLockError::WouldBlock) => Ok(None),
            Err(fs4::TryLockError::Error(error)) => Err(JournalError::Io(format!(
                "cannot lock {}: {error}",
                path.display()
            ))),
        }
    }
}

impl Drop for JournalLock {
    fn drop(&mut self) {
        // Closing the handle releases the lock as well; the explicit call keeps the boundary
        // visible and turns a stuck lock into a log line
        if let Err(e) = fs4::FileExt::unlock(&self.0) {
            warn!(error = %e, "the journal lock did not release");
        }
    }
}

/// The lock file, or `None` when the directory allows none
///
/// A journal opened for reading may sit in a directory this process can only read, and then no
/// lock file can be created beside it: the read goes on, and a write refuses instead of writing
/// without the lock
fn open_lock_file(path: &Path) -> Result<Option<File>, JournalError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => {
            restrict_permissions(&file, path)?;
            Ok(Some(file))
        }
        // The lock file exists but may only be read: the shared lock still works on that handle
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            match File::open(path) {
                Ok(file) => Ok(Some(file)),
                Err(inner)
                    if matches!(
                        inner.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                    ) =>
                {
                    Ok(None)
                }
                Err(inner) => Err(io_err(path, &inner)),
            }
        }
        Err(error) => Err(io_err(path, &error)),
    }
}

/// The shortest line an entry of this schema can produce, with a wide margin below the real one.
/// A file shorter than `max_entries` of these cannot hold that many entries, so the count of the
/// lines is not read at all. A test holds the bound to the schema
const MIN_ENTRY_BYTES: u64 = 128;

/// One pass over the file: the entries, and the lines that did not parse
#[derive(Debug, Default)]
struct Scan {
    entries: Vec<Entry>,
    broken: Vec<String>,
}

/// Journal in a JSONL file
#[derive(Debug, Clone)]
pub struct FileJournal {
    path: PathBuf,
    /// `false` means privacy mode: a line is written without texts and without the audio path
    include_text: bool,
    /// Cipher of the phrase texts (`journal.encrypt`); with `None` the texts are written in
    /// plain text
    cipher: Option<JournalCipher>,
    /// `journal.max_entries` applied by `append`; zero turns the limit off
    max_entries: u32,
    /// `journal.max_size_mb` applied by `append`; zero turns the limit off
    max_size_mb: u32,
}

impl FileJournal {
    /// Open the journal, and create it if needed; the directory is created together with the file
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        Self::open_with(path, true)
    }

    /// The same, with an explicit privacy mode: `include_text = false` removes texts from lines
    pub fn open_with(path: &Path, include_text: bool) -> Result<Self, JournalError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
            }
        }
        let journal = Self {
            path: path.to_path_buf(),
            include_text,
            cipher: None,
            max_entries: 0,
            max_size_mb: 0,
        };
        // An existing journal is opened by nobody at this point: every operation takes the lock
        // and its own handle, so a journal in a directory this process can only read stays
        // readable, and the header is written only where a file is missing
        if !path.exists() {
            // The existence check and the header belong to one critical section: two processes
            // that both saw no file each wrote a header of their own
            let _lock = journal.lock_exclusive()?;
            if !path.exists() {
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|e| io_err(path, &e))?;
                restrict_permissions(&file, path)?;
                let header = Header {
                    schema: SCHEMA_VERSION,
                    created: Utc::now(),
                };
                let line = serde_json::to_string(&header)
                    .map_err(|e| JournalError::Serialize(e.to_string()))?;
                let mut file = file;
                writeln!(file, "{line}").map_err(|e| io_err(path, &e))?;
                file.sync_all().map_err(|e| io_err(path, &e))?;
            }
        }
        Ok(journal)
    }

    /// Journal by the settings: the path, the privacy mode, the cipher and the retention limits
    /// come from one place, so the daemon, the CLI and the GUI open the same file in the same way.
    /// With `journal.encrypt = true`, the key is read or created by `journal.key_source`: in the
    /// OS store (entry `journal-key`) or in the file `journal.key_path`
    pub fn open_for(config: &crate::config::Config) -> Result<Self, JournalError> {
        let path = config
            .journal_path()
            .map_err(|e| JournalError::Io(e.to_string()))?;
        let mut journal = Self::open_with(&path, config.journal.include_text)?;
        journal.max_entries = config.journal.max_entries;
        journal.max_size_mb = config.journal.max_size_mb;
        if !config.journal.encrypt {
            return Ok(journal);
        }
        let cipher = journal.cipher_for(config)?;
        Ok(journal.with_cipher(Some(cipher)))
    }

    /// Key for `journal.encrypt`. A key is created only when the journal holds no ciphertext
    fn cipher_for(&self, config: &crate::config::Config) -> Result<JournalCipher, JournalError> {
        let entry = secrets::JOURNAL_KEY_ENTRY;
        if config
            .journal
            .key_source
            .trim()
            .eq_ignore_ascii_case("keyring")
        {
            let store = secrets::OsKeyring;
            match store.get(entry) {
                Ok(Some(_)) => {}
                // The OS store gives no lock shared by the processes that use it, so two of them
                // that find no entry at the same moment can both write one, and the later write
                // wins. The check below still keeps a new key away from encrypted history
                Ok(None) => self.refuse_new_key(&format!("the OS key store entry {entry}"))?,
                Err(error) => return Err(JournalError::Io(error.to_string())),
            }
            return JournalCipher::from_store(&store, entry)
                .map_err(|e| JournalError::Io(e.to_string()));
        }
        let key_path = config
            .journal_key_path()
            .map_err(|e| JournalError::Io(e.to_string()))?;
        match key_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                std::fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
            }
            _ => {}
        }
        // The key gets a lock file of its own, and the key is read again under it: the caller
        // that lost the race reads the key the winner wrote instead of creating a second one
        let _lock = JournalLock::exclusive(&sibling_lock_path(&key_path))?;
        if !key_path.is_file() {
            self.refuse_new_key(&format!("the key file {}", key_path.display()))?;
        }
        JournalCipher::from_key_file(&key_path).map_err(|e| JournalError::Io(e.to_string()))
    }

    /// A new key cannot read the lines the journal already holds, so it would lock the user out
    /// of their own history without a word. A line that does not parse may hide ciphertext, and
    /// counts as one
    fn refuse_new_key(&self, key: &str) -> Result<(), JournalError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_err(&self.path, &e)),
        };
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || is_header(line) {
                continue;
            }
            let sealed = match serde_json::from_str::<Entry>(line) {
                Err(_) => true,
                Ok(entry) => entry
                    .segments
                    .iter()
                    .map(|segment| &segment.text)
                    .chain(entry.text_raw.iter())
                    .chain(entry.text_final.iter())
                    .any(|field| journal_crypto::is_encrypted(field)),
            };
            if sealed {
                return Err(JournalError::Io(format!(
                    "{key} is missing, but line {} of {} holds encrypted text: restore the key, \
                     or move the journal away before a new key is created",
                    index + 1,
                    self.path.display()
                )));
            }
        }
        Ok(())
    }

    /// Turn on text encryption: with `Some`, the texts of new lines are written as ciphertext
    /// and decrypted on read; lines written in plain text are read without decryption
    #[must_use]
    pub fn with_cipher(mut self, cipher: Option<JournalCipher>) -> Self {
        self.cipher = cipher;
        self
    }

    /// Whether the texts of new lines are encrypted
    pub fn encrypts(&self) -> bool {
        self.cipher.is_some()
    }

    /// Decrypt the text fields of a line. A field that could not be read (a foreign key, changed
    /// bytes, no key at all) becomes empty, and does not show as ciphertext in the history
    fn decrypted(&self, mut entry: Entry) -> Entry {
        let unlock = |field: Option<String>| -> Option<String> {
            let value = field?;
            if !journal_crypto::is_encrypted(&value) {
                return Some(value);
            }
            self.cipher
                .as_ref()
                .and_then(|cipher| cipher.decrypt(&value))
        };
        entry.text_raw = unlock(entry.text_raw);
        entry.text_final = unlock(entry.text_final);
        for segment in &mut entry.segments {
            segment.text = unlock(Some(std::mem::take(&mut segment.text))).unwrap_or_default();
        }
        entry
    }

    fn encrypted(&self, mut entry: Entry, preserve_ciphertext: bool) -> Entry {
        if let Some(cipher) = &self.cipher {
            let seal = |text: &mut String| {
                // Stored ciphertext can use another key; new fields are always plaintext
                if !preserve_ciphertext || !journal_crypto::is_encrypted(text) {
                    *text = cipher.encrypt(text);
                }
            };
            if let Some(text) = &mut entry.text_raw {
                seal(text);
            }
            if let Some(text) = &mut entry.text_final {
                seal(text);
            }
            for segment in &mut entry.segments {
                seal(&mut segment.text);
            }
        }
        entry
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn include_text(&self) -> bool {
        self.include_text
    }

    pub fn set_include_text(&mut self, include_text: bool) {
        self.include_text = include_text;
    }

    /// File where lines that could not be parsed are moved
    pub fn broken_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".broken");
        PathBuf::from(name)
    }

    /// Lock file held for the length of one operation, created beside the journal
    pub fn lock_path(&self) -> PathBuf {
        sibling_lock_path(&self.path)
    }

    fn lock_exclusive(&self) -> Result<JournalLock, JournalError> {
        JournalLock::exclusive(&self.lock_path())
    }

    /// The exclusive lock, or `None` when a writer or a reader holds the journal now
    fn try_lock_exclusive(&self) -> Result<Option<JournalLock>, JournalError> {
        JournalLock::try_acquire(&self.lock_path(), true)
    }

    /// The shared lock, or `None` when a writer holds the journal now
    fn try_lock_shared(&self) -> Result<Option<JournalLock>, JournalError> {
        JournalLock::try_acquire(&self.lock_path(), false)
    }

    /// All entries in order. A damaged line does not fail the load: it goes to `<path>.broken`
    /// when the journal is free at this instant, and waits for the next write otherwise
    pub fn load_all(&self) -> Result<Vec<Entry>, JournalError> {
        self.entries()
    }

    /// Read and decrypt
    ///
    /// A damaged line leaves the journal only under the exclusive lock, and only its holder moves
    /// it: two readers that each wrote under a shared lock put the same line in the quarantine
    /// twice. A read never waits for that lock, because a reader that waited would stop behind a
    /// writer, so a line that cannot move now waits for the next write
    fn entries(&self) -> Result<Vec<Entry>, JournalError> {
        if let Some(_lock) = self.try_lock_exclusive()? {
            let scan = self.scan()?;
            if !scan.broken.is_empty() {
                // The entries a read can show are worth more than the cleanup: a quarantine that
                // fails, by a missing permission or a full disk, does not fail the read
                if let Err(error) = self.quarantine_locked(&scan.broken, &scan.entries) {
                    warn!(error = %error, "the damaged lines stay in the journal");
                }
            }
            return Ok(self.decrypted_all(scan.entries));
        }
        // A writer or another reader holds the journal. A rewrite replaces the file by `rename`,
        // so the snapshot is whole, and only the line a writer appends at this instant can be seen
        // half done: it stays in the file and parses on the next read
        let _shared = self.try_lock_shared()?;
        let scan = self.scan()?;
        if !scan.broken.is_empty() {
            warn!(
                lines = scan.broken.len(),
                "the journal is busy, so the damaged lines wait for the next write"
            );
        }
        Ok(self.decrypted_all(scan.entries))
    }

    /// Decrypt the text fields of every line the file stored
    fn decrypted_all(&self, entries: Vec<Entry>) -> Vec<Entry> {
        entries
            .into_iter()
            .map(|entry| self.decrypted(entry))
            .collect()
    }

    /// The file as it is now, under a lock the caller holds: the entries and the lines that did
    /// not parse. Nothing is written here, so a caller without the exclusive lock can use it
    fn scan(&self) -> Result<Scan, JournalError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Scan::default()),
            Err(e) => return Err(io_err(&self.path, &e)),
        };
        let mut scan = Scan::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) => scan.entries.push(entry),
                Err(_) if is_header(line) => {}
                Err(err) => {
                    warn!(line = index + 1, error = %err, "a journal line did not parse");
                    scan.broken.push(line.to_string());
                }
            }
        }
        Ok(scan)
    }

    /// Entries as the file stores them, under the exclusive lock the caller holds: a damaged line
    /// goes to the quarantine first, so no rewrite puts it back
    ///
    /// A mutation takes the stored entries and not the decrypted ones: a read without a valid key
    /// returns empty text fields, and a rewrite would save those over the history
    fn load_stored(&self) -> Result<Vec<Entry>, JournalError> {
        let scan = self.scan()?;
        if !scan.broken.is_empty() {
            self.quarantine_locked(&scan.broken, &scan.entries)?;
        }
        Ok(scan.entries)
    }

    /// Move damaged lines to the quarantine, and take them out of the journal
    ///
    /// The caller holds the exclusive lock, and both lists come from a scan under that same lock:
    /// a list older than the lock names lines another process has moved already. The move is
    /// idempotent, so a rewrite that failed leaves a journal the next call finishes cleaning
    fn quarantine_locked(&self, broken: &[String], entries: &[Entry]) -> Result<(), JournalError> {
        let path = self.broken_path();
        let pending = self.not_yet_quarantined(broken)?;
        if !pending.is_empty() {
            self.append_quarantine(&pending)?;
        }
        self.rewrite_locked(entries).map_err(|error| {
            JournalError::Io(format!(
                "the damaged lines are in {} but they stay in the journal: {error}",
                path.display()
            ))
        })?;
        warn!(
            lines = broken.len(),
            quarantine = %path.display(),
            "the damaged journal lines moved to the quarantine"
        );
        Ok(())
    }

    /// The lines of `broken` that the quarantine does not hold yet
    ///
    /// The count matters: two equal damaged lines in the journal are two records, so one copy in
    /// the quarantine excuses one occurrence and no more
    fn not_yet_quarantined(&self, broken: &[String]) -> Result<Vec<String>, JournalError> {
        let path = self.broken_path();
        let held = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io_err(&path, &e)),
        };
        let mut counted: HashMap<&str, usize> = HashMap::new();
        for line in held.lines().map(str::trim).filter(|line| !line.is_empty()) {
            *counted.entry(line).or_default() += 1;
        }
        let mut pending = Vec::new();
        for line in broken {
            match counted.get_mut(line.as_str()) {
                Some(count) if *count > 0 => *count -= 1,
                _ => pending.push(line.clone()),
            }
        }
        Ok(pending)
    }

    /// Append lines to the quarantine file. It holds the text of a phrase, so it gets the
    /// permissions of the journal itself
    fn append_quarantine(&self, lines: &[String]) -> Result<(), JournalError> {
        let path = self.broken_path();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| io_err(&path, &e))?;
        restrict_permissions(&file, &path)?;
        for line in lines {
            writeln!(file, "{line}").map_err(|e| io_err(&path, &e))?;
        }
        file.sync_all().map_err(|e| io_err(&path, &e))
    }

    /// Search by substring in the phrase texts, ignoring case
    pub fn search(&self, substring: &str) -> Result<Vec<Entry>, JournalError> {
        Ok(search_entries(&self.entries()?, substring))
    }

    /// Delete one entry by identifier; `false` means no entry had this id
    pub fn delete(&self, id: Uuid) -> Result<bool, JournalError> {
        let _lock = self.lock_exclusive()?;
        let entries = self.load_stored()?;
        let kept: Vec<Entry> = entries.iter().filter(|e| e.id != id).cloned().collect();
        if kept.len() == entries.len() {
            return Ok(false);
        }
        self.rewrite_locked(&kept)?;
        Ok(true)
    }

    /// Clear the whole journal, and leave only the header
    pub fn clear(&self) -> Result<(), JournalError> {
        let _lock = self.lock_exclusive()?;
        self.rewrite_locked(&[])
    }

    /// Rotation: keep at most `max_entries` latest entries and stay within `max_size_mb`
    ///
    /// A zero in either limit turns that limit off. Returns the number of removed entries. The
    /// explicit call keeps exactly what it is given; the retention after an append leaves
    /// headroom, so the same limits cost one rewrite in a thousand phrases rather than one per
    /// phrase
    pub fn rotate(&self, max_entries: u32, max_size_mb: u32) -> Result<usize, JournalError> {
        let _lock = self.lock_exclusive()?;
        self.rotate_locked(max_entries, max_size_mb)
    }

    /// Retention after an append, under the lock the append already holds
    ///
    /// The rotation cuts below the limit instead of down to it. A journal that sits exactly at
    /// `max_entries` crosses the limit with every phrase, so a rotation down to the limit would
    /// rewrite the whole file on every append: tens of milliseconds on ten thousand entries. The
    /// headroom makes that once in a thousand phrases, and the cost is that the limit reads as
    /// "not more than", which is what a history wants anyway
    fn apply_retention_locked(&self) -> Result<(), JournalError> {
        if !self.limits_exceeded()? {
            return Ok(());
        }
        let (entries, size) = (
            retention_target(self.max_entries),
            retention_target(self.max_size_mb),
        );
        self.rotate_locked(entries, size).map(|_| ())
    }

    /// Whether a limit may have been crossed, decided without parsing a line. A full rotation on
    /// every append parses and re-serialises the whole history, which costs tens of milliseconds
    /// on a journal of ten thousand entries, and an append usually crosses no limit at all
    fn limits_exceeded(&self) -> Result<bool, JournalError> {
        let length = match std::fs::metadata(&self.path) {
            Ok(metadata) => metadata.len(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(io_err(&self.path, &e)),
        };
        if self.max_size_mb > 0 && length > u64::from(self.max_size_mb) * 1024 * 1024 {
            return Ok(true);
        }
        if self.max_entries == 0 {
            return Ok(false);
        }
        if length < u64::from(self.max_entries) * MIN_ENTRY_BYTES {
            return Ok(false);
        }
        // Every line of the file is one entry or the header
        Ok(self.count_lines()?.saturating_sub(1) > u64::from(self.max_entries))
    }

    /// Lines in the file, counted by byte rather than parsed: the count only decides whether a
    /// rotation is due, and the rotation counts the entries itself
    ///
    /// The byte scan stays hand-written: a dependency for one pass over a 64 KB buffer is not
    /// worth its place in the lock file
    #[allow(clippy::naive_bytecount)]
    fn count_lines(&self) -> Result<u64, JournalError> {
        use std::io::Read as _;

        let mut file = File::open(&self.path).map_err(|e| io_err(&self.path, &e))?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut lines = 0u64;
        loop {
            let read = file.read(&mut buffer).map_err(|e| io_err(&self.path, &e))?;
            if read == 0 {
                return Ok(lines);
            }
            lines += buffer[..read].iter().filter(|byte| **byte == b'\n').count() as u64;
        }
    }

    /// Rotation under a lock the caller already holds: a second acquisition of the same lock in
    /// one process would wait for itself
    fn rotate_locked(&self, max_entries: u32, max_size_mb: u32) -> Result<usize, JournalError> {
        let entries = self.load_stored()?;
        let total = entries.len();
        let mut kept: &[Entry] = &entries;
        if max_entries > 0 && kept.len() > max_entries as usize {
            kept = &kept[kept.len() - max_entries as usize..];
        }
        if max_size_mb > 0 {
            let limit = u64::from(max_size_mb) * 1024 * 1024;
            // The newest entry stays whatever its size: it is the line that was just written,
            // and a limit smaller than it must not throw the phrase away
            while kept.len() > 1 && serialized_size(kept)? > limit {
                // Cut the oldest: fresh history is worth more to the user
                let drop = (kept.len() / 10).max(1);
                kept = &kept[drop.min(kept.len() - 1)..];
            }
        }
        if kept.len() == total {
            return Ok(0);
        }
        let kept = kept.to_vec();
        self.rewrite_locked(&kept)?;
        Ok(total - kept.len())
    }

    /// Export of all entries to JSONL (the same format as the journal itself, but without the
    /// header)
    pub fn export_jsonl(&self, path: &Path) -> Result<usize, JournalError> {
        let entries = self.entries()?;
        let mut out = String::new();
        for entry in &entries {
            let line =
                serde_json::to_string(entry).map_err(|e| JournalError::Serialize(e.to_string()))?;
            out.push_str(&line);
            out.push('\n');
        }
        std::fs::write(path, out).map_err(|e| io_err(path, &e))?;
        Ok(entries.len())
    }

    /// Export of all entries to CSV
    pub fn export_csv(&self, path: &Path) -> Result<usize, JournalError> {
        let entries = self.entries()?;
        std::fs::write(path, entries_to_csv(&entries)).map_err(|e| io_err(path, &e))?;
        Ok(entries.len())
    }

    /// Atomic rewrite under a lock the caller already holds: a temporary file in the same
    /// directory, then `rename`. A unique name per call keeps two rewrites apart, and the file
    /// removes itself when the write fails
    fn rewrite_locked(&self, entries: &[Entry]) -> Result<(), JournalError> {
        let parent = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| io_err(parent, &e))?;
        let tmp_path = tmp.path().to_path_buf();
        restrict_permissions(tmp.as_file(), &tmp_path)?;
        let mut writer = BufWriter::new(tmp.as_file_mut());
        let header = Header {
            schema: SCHEMA_VERSION,
            created: Utc::now(),
        };
        let line =
            serde_json::to_string(&header).map_err(|e| JournalError::Serialize(e.to_string()))?;
        writeln!(writer, "{line}").map_err(|e| io_err(&tmp_path, &e))?;
        for entry in entries {
            let line = serde_json::to_string(&self.encrypted(entry.clone(), true))
                .map_err(|e| JournalError::Serialize(e.to_string()))?;
            writeln!(writer, "{line}").map_err(|e| io_err(&tmp_path, &e))?;
        }
        writer.flush().map_err(|e| io_err(&tmp_path, &e))?;
        drop(writer);
        tmp.as_file()
            .sync_all()
            .map_err(|e| io_err(&tmp_path, &e))?;
        tmp.persist(&self.path)
            .map(|_| ())
            .map_err(|e| io_err(&self.path, &e.error))
    }
}

impl Journal for FileJournal {
    fn append(&mut self, entry: &Entry) -> Result<(), JournalError> {
        let entry = if self.include_text {
            entry.clone()
        } else {
            entry.clone().without_text()
        };
        let line = serde_json::to_string(&self.encrypted(entry, false))
            .map_err(|e| JournalError::Serialize(e.to_string()))?;
        debug_assert!(!line.contains('\n'), "a journal line must not wrap");
        let _lock = self.lock_exclusive()?;
        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .map_err(|e| io_err(&self.path, &e))?;
            restrict_permissions(&file, &self.path)?;
            writeln!(file, "{line}").map_err(|e| io_err(&self.path, &e))?;
            file.flush().map_err(|e| io_err(&self.path, &e))?;
            file.sync_all().map_err(|e| io_err(&self.path, &e))?;
        }
        // A damaged line waits for the next write, and this is one: the reader that found it may
        // have had to go on without the lock. The pass is one parse of a file the limits bound
        let scan = self.scan()?;
        if !scan.broken.is_empty() {
            self.quarantine_locked(&scan.broken, &scan.entries)
                .map_err(|error| {
                    JournalError::Io(format!(
                        "the entry was written, but the damaged lines stay in the journal: {error}"
                    ))
                })?;
        }
        if self.max_entries == 0 && self.max_size_mb == 0 {
            return Ok(());
        }
        // Retention runs under the lock the append holds: a release and a new acquisition would
        // let another process append in between, and the rotation would cut that line unseen
        self.apply_retention_locked().map_err(|error| {
            JournalError::Io(format!(
                "the entry was written, but the retention did not finish: {error}"
            ))
        })?;
        Ok(())
    }
}

/// A journal that writes nothing: the "do not record" mode (`privacy.no_record_mode`)
#[derive(Debug, Default, Clone, Copy)]
pub struct NullJournal;

impl Journal for NullJournal {
    fn append(&mut self, _entry: &Entry) -> Result<(), JournalError> {
        Ok(())
    }
}

/// Header line: an object with `created` and without `id`, because entries always carry an `id`
fn is_header(line: &str) -> bool {
    serde_json::from_str::<Header>(line).is_ok()
}

/// Mode 0600: only the owner reads the journal
#[cfg(unix)]
fn restrict_permissions(file: &File, path: &Path) -> Result<(), JournalError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = file.metadata().map_err(|e| io_err(path, &e))?.permissions();
    if perms.mode() & 0o777 != 0o600 {
        perms.set_mode(0o600);
        file.set_permissions(perms).map_err(|e| io_err(path, &e))?;
    }
    Ok(())
}

#[cfg(not(unix))]
// Both versions must have one signature: on unix this function returns an error
#[allow(clippy::unnecessary_wraps)]
fn restrict_permissions(_file: &File, _path: &Path) -> Result<(), JournalError> {
    Ok(())
}

/// What a limit leaves after the retention cut it: nine tenths of it, and never zero
fn retention_target(limit: u32) -> u32 {
    if limit == 0 {
        return 0;
    }
    (limit.saturating_mul(9) / 10).max(1)
}

fn serialized_size(entries: &[Entry]) -> Result<u64, JournalError> {
    let mut size = 0u64;
    for entry in entries {
        let line =
            serde_json::to_string(entry).map_err(|e| JournalError::Serialize(e.to_string()))?;
        size += line.len() as u64 + 1;
    }
    Ok(size)
}

/// Search by substring in the raw and the final text, ignoring case
pub fn search_entries(entries: &[Entry], substring: &str) -> Vec<Entry> {
    let needle = substring.to_lowercase();
    if needle.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|entry| {
            let hit = |field: &Option<String>| {
                field
                    .as_deref()
                    .is_some_and(|text| text.to_lowercase().contains(&needle))
            };
            hit(&entry.text_final) || hit(&entry.text_raw)
        })
        .cloned()
        .collect()
}

/// Application names that occur in the journal, alphabetical and unique
pub fn apps(entries: &[Entry]) -> Vec<String> {
    let mut names: Vec<String> = entries
        .iter()
        .filter_map(|entry| entry.app.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Filter by application class, ignoring case
pub fn filter_by_app(entries: &[Entry], app: &str) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| {
            entry
                .app
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(app))
        })
        .cloned()
        .collect()
}

/// Filter by time: the bounds are inclusive, and `None` means the bound is not set
pub fn filter_by_date(
    entries: &[Entry],
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Vec<Entry> {
    entries
        .iter()
        .filter(|entry| since.is_none_or(|from| entry.ts >= from))
        .filter(|entry| until.is_none_or(|to| entry.ts <= to))
        .cloned()
        .collect()
}

/// CSV form of the history: a header and one line per entry
pub fn entries_to_csv(entries: &[Entry]) -> String {
    let mut out =
        String::from("ts,id,app,style,words,audio_secs,wpm,llm_used,latency_total_ms,text_final\n");
    for entry in entries {
        let row = [
            entry.ts.to_rfc3339(),
            entry.id.to_string(),
            entry.app.clone().unwrap_or_default(),
            entry.style.clone(),
            entry.words.to_string(),
            format!("{:.2}", entry.audio_secs),
            entry.wpm.map(|w| format!("{w:.1}")).unwrap_or_default(),
            entry.llm_used.to_string(),
            entry.latency_ms.total.to_string(),
            entry.text_final.clone().unwrap_or_default(),
        ];
        let row: Vec<String> = row.iter().map(|field| csv_field(field)).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Escaping of a CSV field by RFC 4180: quotes are doubled, and the field is quoted when needed
pub fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Sample entry for the tests of the journal, the statistics and the pipeline
#[cfg(test)]
pub(crate) fn test_entry(ts: &str, words: u32, audio_secs: f32, app: &str) -> Entry {
    use crate::domain::entry::{LatencyMs, Mode, Source};

    let ts = DateTime::parse_from_rfc3339(ts)
        .expect("the sample time parses")
        .with_timezone(&Utc);
    Entry {
        schema: SCHEMA_VERSION,
        id: Uuid::new_v4(),
        ts,
        session_id: Uuid::nil(),
        mode: Mode::Dictation,
        source: Source::Mic,
        snippet: false,
        app: Some(app.to_string()),
        language: Some("ru".into()),
        audio_secs,
        words,
        wpm: Entry::wpm_for(words, audio_secs),
        style: "cleanup".into(),
        stt_engine: "fake".into(),
        stt_model: "fake".into(),
        llm_provider: None,
        llm_model: None,
        llm_used: false,
        local_llm: true,
        dict_hits: 0,
        inject_method: Some("clipboard-only".into()),
        latency_ms: LatencyMs {
            stt: 400,
            rules: 2,
            total: 500,
            ..Default::default()
        },
        tokens: None,
        llm_cost_usd: None,
        llm_calls: 0,
        llm_unpriced_calls: 0,
        stt_cost_usd: None,
        stt_unpriced: false,
        error: None,
        text_raw: Some(format!("сырой текст {words}")),
        text_final: Some(format!("Итоговый текст {words}.")),
        audio_path: None,
        segments: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> (tempfile::TempDir, FileJournal) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data").join("journal.jsonl");
        let journal = FileJournal::open(&path).unwrap();
        (directory, journal)
    }

    /// A line written past the journal, as a torn append or a foreign writer leaves it
    fn append_raw(journal: &FileJournal, line: &str) {
        let mut file = OpenOptions::new()
            .append(true)
            .open(journal.path())
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }

    fn broken_lines(journal: &FileJournal) -> Vec<String> {
        match std::fs::read_to_string(journal.broken_path()) {
            Ok(text) => text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!(
                "the quarantine {} is unreadable: {error}",
                journal.broken_path().display()
            ),
        }
    }

    #[test]
    fn the_shortest_entry_is_longer_than_the_cheap_count_bound() {
        // `limits_exceeded` skips the line count of a file too short to hold `max_entries`
        // entries, and that shortcut rests on this bound
        let mut entry = test_entry("2026-09-05T10:00:00Z", 0, 0.0, "");
        entry.app = None;
        entry.language = None;
        entry.llm_provider = None;
        entry.llm_model = None;
        entry.wpm = None;
        entry.inject_method = None;
        entry.tokens = None;
        entry.llm_cost_usd = None;
        entry.error = None;
        entry.text_raw = None;
        entry.text_final = None;
        entry.audio_path = None;
        entry.segments = Vec::new();
        entry.latency_ms = crate::domain::entry::LatencyMs::default();
        entry.style = String::new();
        entry.stt_engine = String::new();
        entry.stt_model = String::new();
        let line = serde_json::to_string(&entry).unwrap();
        assert!(
            line.len() as u64 > MIN_ENTRY_BYTES,
            "an entry line is {} bytes, the bound claims more than {MIN_ENTRY_BYTES}",
            line.len()
        );
    }

    #[test]
    fn each_entry_is_one_json_line() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        journal
            .append(&test_entry("2026-09-05T10:01:00Z", 20, 8.0, "kitty"))
            .unwrap();
        let text = std::fs::read_to_string(journal.path()).unwrap();
        // The header plus two entries
        assert_eq!(text.lines().count(), 3);
        assert_eq!(journal.load_all().unwrap().len(), 2);
    }

    #[test]
    fn entries_survive_reopening_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.jsonl");
        {
            let mut journal = FileJournal::open(&path).unwrap();
            journal
                .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
                .unwrap();
        }
        let reopened = FileJournal::open(&path).unwrap();
        let entries = reopened.load_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].words, 10);
    }

    #[test]
    fn header_line_is_not_an_entry_and_not_broken() {
        let (_dir, journal) = journal();
        let text = std::fs::read_to_string(journal.path()).unwrap();
        assert!(text.contains("\"created\""), "{text}");
        assert!(journal.load_all().unwrap().is_empty());
        assert!(!journal.broken_path().exists());
    }

    #[test]
    fn corrupt_line_is_quarantined_and_the_rest_still_loads() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(journal.path())
            .unwrap();
        writeln!(file, "{{this is not json").unwrap();
        drop(file);
        journal
            .append(&test_entry("2026-09-05T10:02:00Z", 30, 9.0, "kitty"))
            .unwrap();

        let entries = journal.load_all().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].words, 30);
        let broken = std::fs::read_to_string(journal.broken_path()).unwrap();
        assert!(broken.contains("this is not json"), "{broken}");
    }

    #[test]
    fn two_readers_move_a_damaged_line_to_the_quarantine_once() {
        use std::sync::{Arc, Barrier};

        let (_dir, mut journal) = journal();
        let kept = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        journal.append(&kept).unwrap();
        append_raw(&journal, "{not json");
        let path = journal.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let journal = FileJournal::open(&path).unwrap();
                    barrier.wait();
                    journal.load_all().unwrap()
                })
            })
            .collect();

        for reader in readers {
            let entries = reader.join().unwrap();
            assert_eq!(
                entries.len(),
                1,
                "a reader lost the whole line: {entries:#?}"
            );
            assert_eq!(entries[0].id, kept.id);
            assert_eq!(entries[0].words, 10);
        }
        assert_eq!(broken_lines(&journal), vec!["{not json".to_string()]);
        let text = std::fs::read_to_string(journal.path()).unwrap();
        assert!(
            !text.contains("not json"),
            "the journal still holds the damaged line: {text}"
        );
    }

    #[test]
    fn a_quarantined_line_is_not_moved_again_by_the_next_read() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        append_raw(&journal, "{not json");

        assert_eq!(journal.load_all().unwrap().len(), 1);
        assert_eq!(journal.load_all().unwrap().len(), 1);
        assert_eq!(journal.search("Итоговый").unwrap().len(), 1);
        assert_eq!(broken_lines(&journal), vec!["{not json".to_string()]);
        assert_eq!(journal.load_all().unwrap().len(), 1);
        assert_eq!(broken_lines(&journal).len(), 1);
    }

    #[test]
    fn two_equal_damaged_lines_leave_two_records_in_the_quarantine() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        append_raw(&journal, "{not json");
        append_raw(&journal, "{not json");

        assert_eq!(journal.load_all().unwrap().len(), 1);
        assert_eq!(
            broken_lines(&journal),
            vec!["{not json".to_string(), "{not json".to_string()]
        );
        // The second read finds nothing left to move, so it adds nothing
        assert_eq!(journal.load_all().unwrap().len(), 1);
        assert_eq!(broken_lines(&journal).len(), 2);
    }

    #[test]
    fn a_copy_already_in_the_quarantine_is_not_written_again() {
        // The rewrite that takes the line out of the journal fails after the quarantine write:
        // the next attempt finishes the job without a second copy of the same line
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        append_raw(&journal, "{not json");
        std::fs::write(journal.broken_path(), "{not json\n").unwrap();

        assert_eq!(journal.load_all().unwrap().len(), 1);
        assert_eq!(broken_lines(&journal), vec!["{not json".to_string()]);
        assert!(!std::fs::read_to_string(journal.path())
            .unwrap()
            .contains("not json"));
    }

    #[test]
    fn a_read_while_a_writer_holds_the_journal_keeps_every_whole_line() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (_dir, mut journal) = journal();
        let first = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        let second = test_entry("2026-09-05T10:01:00Z", 20, 8.0, "kitty");
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();
        append_raw(&journal, "{not json");

        // A writer in the middle of its work holds the journal for the whole read
        let reader_journal = journal.clone();
        let writer = journal.lock_exclusive().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            sender.send(reader_journal.load_all()).unwrap();
        });
        let entries = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("the read waited for the writer instead of reading a snapshot")
            .unwrap();
        assert_eq!(entries.len(), 2, "a reader lost a whole line: {entries:#?}");
        assert_eq!(entries[0].id, first.id);
        assert_eq!(entries[1].id, second.id);
        assert!(
            !journal.broken_path().exists(),
            "a read moved a line without the exclusive lock"
        );
        drop(writer);
        reader.join().unwrap();

        // The damaged line waits for the next write, and that write moves it once
        journal
            .append(&test_entry("2026-09-05T10:02:00Z", 30, 9.0, "kitty"))
            .unwrap();
        assert_eq!(broken_lines(&journal), vec!["{not json".to_string()]);
        assert_eq!(journal.load_all().unwrap().len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn the_quarantine_file_is_readable_only_by_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, journal) = journal();
        append_raw(&journal, "{not json");
        assert!(journal.load_all().unwrap().is_empty());
        let mode = std::fs::metadata(journal.broken_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[test]
    fn privacy_mode_keeps_metrics_and_drops_texts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.jsonl");
        let mut journal = FileJournal::open_with(&path, false).unwrap();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("Итоговый текст"), "{text}");
        let entries = journal.load_all().unwrap();
        assert_eq!(entries[0].words, 10);
        assert_eq!(entries[0].text_final, None);
    }

    #[test]
    fn null_journal_writes_nothing_anywhere() {
        let mut journal = NullJournal;
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
    }

    #[test]
    fn search_matches_substring_ignoring_case() {
        let (_dir, mut journal) = journal();
        let mut first = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        first.text_final = Some("Собрание переносится".into());
        let mut second = test_entry("2026-09-05T10:01:00Z", 12, 5.0, "firefox");
        second.text_final = Some("Отчёт готов".into());
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();

        let found = journal.search("СОБРАНИЕ").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text_final.as_deref(), Some("Собрание переносится"));
        assert_eq!(journal.search("").unwrap().len(), 2);
    }

    #[test]
    fn filters_select_by_app_and_by_date_range() {
        let entries = vec![
            test_entry("2026-09-01T10:00:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-05T10:00:00Z", 12, 5.0, "Firefox"),
            test_entry("2026-09-09T10:00:00Z", 14, 6.0, "kitty"),
        ];
        assert_eq!(filter_by_app(&entries, "firefox").len(), 1);
        assert_eq!(filter_by_app(&entries, "kitty").len(), 2);

        let since = DateTime::parse_from_rfc3339("2026-09-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until = DateTime::parse_from_rfc3339("2026-09-05T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);
        let day = filter_by_date(&entries, Some(since), Some(until));
        assert_eq!(day.len(), 1);
        assert_eq!(day[0].words, 12);
        assert_eq!(filter_by_date(&entries, Some(since), None).len(), 2);
    }

    #[test]
    fn delete_removes_only_the_named_entry() {
        let (_dir, mut journal) = journal();
        let first = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        let second = test_entry("2026-09-05T10:01:00Z", 12, 5.0, "kitty");
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();

        assert!(journal.delete(first.id).unwrap());
        let left = journal.load_all().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, second.id);
        assert!(!journal.delete(first.id).unwrap());
    }

    #[test]
    fn clear_empties_the_journal_but_keeps_it_usable() {
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        journal.clear().unwrap();
        assert!(journal.load_all().unwrap().is_empty());
        journal
            .append(&test_entry("2026-09-05T11:00:00Z", 7, 3.0, "kitty"))
            .unwrap();
        assert_eq!(journal.load_all().unwrap().len(), 1);
    }

    #[test]
    fn rotation_keeps_the_newest_entries() {
        let (_dir, mut journal) = journal();
        for minute in 0..10 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        assert_eq!(journal.rotate(4, 0).unwrap(), 6);
        let left = journal.load_all().unwrap();
        assert_eq!(left.len(), 4);
        assert_eq!(left[0].words, 7);
        assert_eq!(left[3].words, 10);
        // A repeated rotation removes nothing
        assert_eq!(journal.rotate(4, 0).unwrap(), 0);
    }

    #[test]
    fn rotation_by_size_drops_the_oldest_until_it_fits() {
        let (_dir, mut journal) = journal();
        for minute in 0..20 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        // One entry is far below a megabyte, so the limit of 1 MB cuts nothing
        assert_eq!(journal.rotate(0, 1).unwrap(), 0);
        assert_eq!(journal.load_all().unwrap().len(), 20);
    }

    #[test]
    fn zero_limits_disable_rotation() {
        let (_dir, mut journal) = journal();
        for minute in 0..5 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        assert_eq!(journal.rotate(0, 0).unwrap(), 0);
        assert_eq!(journal.load_all().unwrap().len(), 5);
    }

    #[test]
    fn export_writes_jsonl_and_csv() {
        let (directory, mut journal) = journal();
        let mut entry = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        entry.text_final = Some("Привет, \"мир\"".into());
        journal.append(&entry).unwrap();

        let jsonl = directory.path().join("out.jsonl");
        assert_eq!(journal.export_jsonl(&jsonl).unwrap(), 1);
        let text = std::fs::read_to_string(&jsonl).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(serde_json::from_str::<Entry>(text.trim()).is_ok());

        let csv = directory.path().join("out.csv");
        assert_eq!(journal.export_csv(&csv).unwrap(), 1);
        let csv = std::fs::read_to_string(&csv).unwrap();
        let mut lines = csv.lines();
        assert_eq!(
            lines.next().unwrap(),
            "ts,id,app,style,words,audio_secs,wpm,llm_used,latency_total_ms,text_final"
        );
        let row = lines.next().unwrap();
        assert!(row.contains("kitty"), "{row}");
        assert!(row.contains("\"Привет, \"\"мир\"\"\""), "{row}");
    }

    #[test]
    fn csv_field_escapes_only_when_needed() {
        assert_eq!(csv_field("kitty"), "kitty");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[cfg(unix)]
    #[test]
    fn a_journal_in_a_read_only_directory_is_read_without_a_lock() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.jsonl");
        {
            let mut writer = FileJournal::open(&path).unwrap();
            writer
                .append(&test_entry("2026-09-05T12:00:00Z", 3, 2.0, "kitty"))
                .unwrap();
        }
        // No lock file existed while the journal was written: the directory is made read-only
        // afterwards, and nothing can create one now
        std::fs::remove_file(path.with_file_name("journal.jsonl.lock")).unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let mut reader = FileJournal::open(&path).unwrap();
        let entries = reader
            .load_all()
            .expect("a read-only directory must not fail a read");
        assert_eq!(entries.len(), 1);
        assert!(
            !path.with_file_name("journal.jsonl.lock").exists(),
            "a lock file appeared in a directory that allows none"
        );
        assert!(
            reader
                .append(&test_entry("2026-09-05T12:01:00Z", 4, 2.0, "kitty"))
                .is_err(),
            "a write without a lock must be refused, not done blind"
        );
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn journal_file_is_readable_only_by_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, journal) = journal();
        let mode = std::fs::metadata(journal.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    #[test]
    fn encrypted_journal_keeps_no_plaintext_on_disk_and_reads_back_with_the_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.jsonl");
        let key = [3u8; journal_crypto::KEY_LEN];
        let mut journal = FileJournal::open(&path)
            .unwrap()
            .with_cipher(Some(JournalCipher::from_key(&key)));
        let mut entry = test_entry("2026-09-05T12:00:00Z", 4, 3.0, "kitty");
        entry.text_raw = Some("сырой секретный текст".into());
        entry.text_final = Some("Итоговый секретный текст.".into());
        journal.append(&entry).unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("секретный"),
            "the text on disk is in plain text: {on_disk}"
        );
        assert!(on_disk.contains("enc1:"), "{on_disk}");
        // The other fields stay in plain text: statistics and filters work without the key
        assert!(on_disk.contains("\"words\""), "{on_disk}");

        let loaded = journal.load_all().unwrap();
        assert_eq!(
            loaded[0].text_final.as_deref(),
            Some("Итоговый секретный текст.")
        );
        assert_eq!(loaded[0].text_raw.as_deref(), Some("сырой секретный текст"));

        // Without the key the history shows empty fields, not ciphertext
        let without_key = FileJournal::open(&path).unwrap();
        let blind = without_key.load_all().unwrap();
        assert_eq!(blind[0].text_final, None);
        assert_eq!(blind[0].text_raw, None);
    }

    fn private_entry() -> Entry {
        let mut entry = test_entry("2026-09-05T12:00:00Z", 4, 3.0, "fixture");
        entry.text_raw = Some("fixture-private-raw".into());
        entry.text_final = Some("fixture-private-final".into());
        entry.audio_path = Some("fixture.wav".into());
        entry.segments = vec![crate::domain::stt::Segment {
            start_ms: 10,
            end_ms: 100,
            text: "fixture-private-segment".into(),
        }];
        entry
    }

    fn stored_entries(journal: &FileJournal) -> Vec<Entry> {
        std::fs::read_to_string(journal.path())
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    fn assert_sealed(journal: &FileJournal, original: &Entry) {
        let disk = std::fs::read_to_string(journal.path()).unwrap();
        for text in [
            original.text_raw.as_deref().unwrap(),
            original.text_final.as_deref().unwrap(),
            original.segments[0].text.as_str(),
        ] {
            assert!(!disk.contains(text), "plaintext leaked: {text}");
        }
        for entry in stored_entries(journal) {
            assert!(journal_crypto::is_encrypted(
                entry.text_raw.as_ref().unwrap()
            ));
            assert!(journal_crypto::is_encrypted(
                entry.text_final.as_ref().unwrap()
            ));
            assert!(journal_crypto::is_encrypted(&entry.segments[0].text));
        }
    }

    #[test]
    fn encryption_covers_segments_and_hides_unavailable_text() {
        let (_dir, journal) = journal();
        let mut journal = journal.with_cipher(Some(JournalCipher::from_key(&[3; 32])));
        let entry = private_entry();
        journal.append(&entry).unwrap();
        assert_sealed(&journal, &entry);
        assert_eq!(journal.load_all().unwrap(), vec![entry.clone()]);
        for cipher in [None, Some(JournalCipher::from_key(&[4; 32]))] {
            let blind = FileJournal::open(journal.path())
                .unwrap()
                .with_cipher(cipher);
            let loaded = blind.load_all().unwrap();
            assert_eq!(loaded[0].text_raw, None);
            assert_eq!(loaded[0].text_final, None);
            assert_eq!(loaded[0].segments[0].text, "");
            assert_eq!(loaded[0].segments[0].start_ms, 10);
            assert_eq!(loaded[0].segments[0].end_ms, 100);
        }
    }

    #[test]
    fn append_encrypts_new_text_even_with_a_ciphertext_prefix() {
        let cipher = JournalCipher::from_key(&[3; 32]);
        for text in [
            "enc1:".to_string(),
            "enc1:fixture-private-plaintext".to_string(),
            cipher.encrypt("fixture-private-inner"),
        ] {
            let (_dir, journal) = journal();
            let mut journal = journal.with_cipher(Some(cipher.clone()));
            let mut entry = private_entry();
            entry.text_raw = Some(text.clone());
            entry.text_final = Some(text.clone());
            entry.segments[0].text = text;
            journal.append(&entry).unwrap();
            let stored = &stored_entries(&journal)[0];
            assert_ne!(stored.text_raw, entry.text_raw);
            assert_ne!(stored.text_final, entry.text_final);
            assert_ne!(stored.segments[0].text, entry.segments[0].text);
            assert_eq!(journal.load_all().unwrap(), vec![entry]);
        }
    }

    #[test]
    fn encrypted_delete_and_rotate_preserve_text_with_any_key_access() {
        for cipher in [
            None,
            Some(JournalCipher::from_key(&[3; 32])),
            Some(JournalCipher::from_key(&[4; 32])),
        ] {
            let (_dir, journal) = journal();
            let mut writer = journal.with_cipher(Some(JournalCipher::from_key(&[3; 32])));
            let first = private_entry();
            let mut second = first.clone();
            second.id = Uuid::new_v4();
            let mut third = first.clone();
            third.id = Uuid::new_v4();
            for entry in [&first, &second, &third] {
                writer.append(entry).unwrap();
            }
            let before = stored_entries(&writer);
            let editor = FileJournal::open_with(writer.path(), false)
                .unwrap()
                .with_cipher(cipher);
            assert!(editor.delete(first.id).unwrap());
            assert_eq!(stored_entries(&editor), before[1..]);
            assert_eq!(editor.rotate(1, 0).unwrap(), 1);
            assert_eq!(stored_entries(&editor), before[2..]);
            assert_sealed(&editor, &third);
            assert_eq!(writer.load_all().unwrap(), vec![third]);
        }
    }

    #[test]
    fn malformed_ciphertext_stays_hidden_and_survives_rewrite() {
        let (_dir, mut journal) = journal();
        let first = private_entry();
        journal.append(&first).unwrap();
        let mut damaged = first.clone();
        damaged.id = Uuid::new_v4();
        damaged.text_raw = Some("enc1:not-hex-raw".into());
        damaged.text_final = Some("enc1:not-hex-final".into());
        damaged.segments[0].text = "enc1:not-hex-segment".into();
        journal.append(&damaged).unwrap();
        for cipher in [None, Some(JournalCipher::from_key(&[3; 32]))] {
            let reader = journal.clone().with_cipher(cipher);
            let hidden = &reader.load_all().unwrap()[1];
            assert_eq!(hidden.text_raw, None);
            assert_eq!(hidden.text_final, None);
            assert_eq!(hidden.segments[0].text, "");
        }
        let encrypted = journal.with_cipher(Some(JournalCipher::from_key(&[3; 32])));
        assert!(encrypted.delete(first.id).unwrap());
        assert_eq!(stored_entries(&encrypted), vec![damaged]);
    }

    #[test]
    fn encrypted_rewrite_seals_legacy_plaintext_after_delete_and_rotate() {
        for rotate in [false, true] {
            let (_dir, mut journal) = journal();
            let first = private_entry();
            let mut second = first.clone();
            second.id = Uuid::new_v4();
            journal.append(&first).unwrap();
            journal.append(&second).unwrap();
            let journal = journal.with_cipher(Some(JournalCipher::from_key(&[3; 32])));
            if rotate {
                assert_eq!(journal.rotate(1, 0).unwrap(), 1);
            } else {
                assert!(journal.delete(first.id).unwrap());
            }
            assert_sealed(&journal, &second);
            assert_eq!(journal.load_all().unwrap(), vec![second]);
        }
    }

    #[test]
    fn encryption_preserves_existing_ciphertext_and_plaintext_history() {
        let (_dir, mut plain) = journal();
        let original = private_entry();
        plain.append(&original).unwrap();
        assert_eq!(plain.load_all().unwrap(), vec![original.clone()]);
        let mut encrypted = plain
            .clone()
            .with_cipher(Some(JournalCipher::from_key(&[3; 32])));
        let mut second = original.clone();
        second.id = Uuid::new_v4();
        encrypted.append(&second).unwrap();
        let sealed = stored_entries(&encrypted)[1].clone();
        let mut copy = original.clone();
        copy.id = Uuid::new_v4();
        plain.append(&copy).unwrap();
        assert_eq!(stored_entries(&encrypted)[2], copy);
        assert_eq!(encrypted.load_all().unwrap()[0], original);
        assert!(plain.delete(copy.id).unwrap());
        assert_eq!(stored_entries(&plain)[0], original);
        assert_eq!(stored_entries(&plain)[1], sealed);
        assert_eq!(encrypted.load_all().unwrap(), vec![original, second]);
    }

    #[test]
    fn privacy_mode_strips_new_text_but_preserves_history_on_rewrite() {
        let (_dir, mut journal) = journal();
        let first = private_entry();
        let mut second = first.clone();
        second.id = Uuid::new_v4();
        journal = journal.with_cipher(Some(JournalCipher::from_key(&[3; 32])));
        journal.append(&first).unwrap();
        journal.append(&second).unwrap();
        let stored = stored_entries(&journal)[1].clone();
        let mut blind = FileJournal::open_with(journal.path(), false).unwrap();
        assert!(blind.delete(first.id).unwrap());
        assert_eq!(stored_entries(&blind), vec![stored]);
        assert_eq!(journal.load_all().unwrap(), vec![second]);
        blind.append(&first).unwrap();
        assert_eq!(journal.load_all().unwrap()[1], first.without_text());
    }

    #[test]
    fn open_for_creates_the_key_file_next_to_the_journal_only_when_asked() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::default();
        config.journal.path = directory.path().join("journal.jsonl").display().to_string();
        assert!(!FileJournal::open_for(&config).unwrap().encrypts());
        assert!(!directory.path().join("journal.key").exists());

        config.journal.encrypt = true;
        assert!(FileJournal::open_for(&config).unwrap().encrypts());
        assert!(directory.path().join("journal.key").is_file());
    }

    #[test]
    fn loading_a_missing_file_is_an_empty_history() {
        let directory = tempfile::tempdir().unwrap();
        let journal = FileJournal {
            path: directory.path().join("absent.jsonl"),
            include_text: true,
            cipher: None,
            max_entries: 0,
            max_size_mb: 0,
        };
        assert!(journal.load_all().unwrap().is_empty());
    }

    // Journal shared by several processes: the daemon appends while the CLI and the GUI delete

    fn config_in(directory: &Path) -> crate::config::Config {
        let mut config = crate::config::Config::default();
        config.journal.path = directory.join("journal.jsonl").display().to_string();
        config.journal.key_path = directory.join("journal.key").display().to_string();
        config
    }

    fn ids(journal: &FileJournal) -> Vec<Uuid> {
        journal
            .load_all()
            .unwrap()
            .iter()
            .map(|entry| entry.id)
            .collect()
    }

    #[test]
    fn an_append_during_a_deletion_is_not_lost() {
        use std::sync::{Arc, Barrier};

        let (_dir, mut journal) = journal();
        // A long history widens the window between reading the file and replacing it
        for minute in 0..200 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T{:02}:{:02}:00Z", 10 + minute / 60, minute % 60),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        let old = ids(&journal);
        let path = journal.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));

        let writer = {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut journal = FileJournal::open(&path).unwrap();
                barrier.wait();
                let mut written = Vec::new();
                for minute in 0..30 {
                    let entry = test_entry(
                        &format!("2026-09-06T11:{minute:02}:00Z"),
                        minute + 1,
                        4.0,
                        "kitty",
                    );
                    journal.append(&entry).unwrap();
                    written.push(entry.id);
                }
                written
            })
        };
        let deleter = {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let journal = FileJournal::open(&path).unwrap();
                barrier.wait();
                for id in old {
                    journal.delete(id).unwrap();
                }
            })
        };

        let written = writer.join().unwrap();
        deleter.join().unwrap();
        let left = ids(&FileJournal::open(&path).unwrap());
        let lost: Vec<Uuid> = written
            .iter()
            .filter(|id| !left.contains(id))
            .copied()
            .collect();
        assert!(
            lost.is_empty(),
            "a concurrent deletion dropped appended entries: {lost:?}"
        );
    }

    #[test]
    fn concurrent_open_of_a_missing_journal_writes_one_header() {
        use std::sync::{Arc, Barrier};

        for round in 0..8 {
            let directory = tempfile::tempdir().unwrap();
            let path = directory
                .path()
                .join("data")
                .join(format!("journal-{round}.jsonl"));
            let barrier = Arc::new(Barrier::new(6));
            let handles: Vec<_> = (0..6)
                .map(|_| {
                    let path = path.clone();
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait();
                        FileJournal::open(&path).unwrap();
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            assert_eq!(
                lines.len(),
                1,
                "round {round} wrote {} lines: {text}",
                lines.len()
            );
            assert!(is_header(lines[0]), "round {round}: {text}");
        }
    }

    #[test]
    fn concurrent_rewrites_do_not_collide() {
        use std::sync::{Arc, Barrier};

        let (directory, journal) = journal();
        let path = journal.path().to_path_buf();
        for round in 0..6 {
            let mut filler = FileJournal::open(&path).unwrap();
            for minute in 0..60 {
                let mut entry = test_entry(
                    &format!("2026-09-05T12:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                );
                // Large lines keep the two rewrites writing at the same time
                entry.text_final = Some("x".repeat(30 * 1024));
                filler.append(&entry).unwrap();
            }
            let barrier = Arc::new(Barrier::new(2));
            let clearer = {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    FileJournal::open(&path).unwrap().clear()
                })
            };
            let rotator = {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    FileJournal::open(&path).unwrap().rotate(10, 0)
                })
            };
            clearer.join().unwrap().unwrap();
            rotator.join().unwrap().unwrap();

            // Serialised either way, both operations leave an empty journal: a clear that runs
            // first leaves nothing for the rotation to keep
            let text = std::fs::read_to_string(&path).unwrap();
            let headers = text.lines().filter(|line| is_header(line)).count();
            assert_eq!(headers, 1, "round {round}: {text}");
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                assert!(
                    serde_json::from_str::<Entry>(line).is_ok() || is_header(line),
                    "round {round} left a torn line: {line}"
                );
            }
            let left = FileJournal::open(&path).unwrap().load_all().unwrap();
            assert!(left.is_empty(), "round {round} kept {} entries", left.len());
        }
        // No rewrite leaves a temporary file behind
        let stray: Vec<String> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".tmp"))
            .collect();
        assert!(stray.is_empty(), "temporary files left: {stray:?}");
    }

    #[test]
    fn a_rewrite_uses_a_temporary_file_of_its_own() {
        // A rewrite that crashed under the old fixed name leaves a stump; a new rewrite must not
        // reuse or truncate it, or two writers would share one file again
        let (_dir, mut journal) = journal();
        journal
            .append(&test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"))
            .unwrap();
        let mut name = journal.path().as_os_str().to_os_string();
        name.push(".tmp");
        let stray = PathBuf::from(name);
        std::fs::write(&stray, "stump").unwrap();

        journal.clear().unwrap();

        assert_eq!(std::fs::read_to_string(&stray).unwrap(), "stump");
        assert!(journal.load_all().unwrap().is_empty());
    }

    #[test]
    fn open_for_applies_the_entry_limit_to_every_append() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.max_entries = 3;
        config.journal.max_size_mb = 0;
        let mut journal = FileJournal::open_for(&config).unwrap();
        for minute in 0..10 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        let left = journal.load_all().unwrap();
        // The limit reads as "not more than": the retention cuts a tenth below it, so a journal
        // that sits at the cap is not rewritten with every phrase
        assert!(left.len() <= 3, "more than the limit: {left:#?}");
        assert_eq!(left.last().unwrap().words, 10, "the newest entry is kept");
    }

    /// The point of the headroom: an append within the limit touches no other line, and a
    /// rewrite would replace the file by `rename`, which gives it another inode
    #[cfg(unix)]
    #[test]
    fn the_retention_leaves_headroom_so_the_file_is_not_rewritten_every_phrase() {
        use std::os::unix::fs::MetadataExt as _;
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.max_entries = 10;
        config.journal.max_size_mb = 0;
        let mut journal = FileJournal::open_for(&config).unwrap();
        let append = |journal: &mut FileJournal, minute: u32| {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        };
        for minute in 0..11 {
            append(&mut journal, minute);
        }
        let inode = std::fs::metadata(journal.path()).unwrap().ino();
        assert_eq!(
            journal.load_all().unwrap().len(),
            9,
            "the cut leaves a tenth of the limit free"
        );
        // Up to the limit again: nothing is rewritten
        append(&mut journal, 11);
        assert_eq!(
            std::fs::metadata(journal.path()).unwrap().ino(),
            inode,
            "a phrase within the limit rewrote the whole journal"
        );
        assert_eq!(journal.load_all().unwrap().len(), 10);
    }

    #[test]
    fn a_plain_open_holds_no_limits() {
        // `open` and `open_with` pass zero limits, so the callers that never read the settings
        // keep the history they had
        let (_dir, mut journal) = journal();
        for minute in 0..12 {
            journal
                .append(&test_entry(
                    &format!("2026-09-05T10:{minute:02}:00Z"),
                    minute + 1,
                    4.0,
                    "kitty",
                ))
                .unwrap();
        }
        assert_eq!(journal.load_all().unwrap().len(), 12);
    }

    #[test]
    fn open_for_keeps_the_history_within_the_size_limit() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.max_entries = 0;
        config.journal.max_size_mb = 1;
        let mut journal = FileJournal::open_for(&config).unwrap();
        let mut written = Vec::new();
        for minute in 0..12 {
            let mut entry = test_entry(
                &format!("2026-09-05T10:{minute:02}:00Z"),
                minute + 1,
                4.0,
                "kitty",
            );
            entry.text_final = Some("x".repeat(200 * 1024));
            journal.append(&entry).unwrap();
            written.push(entry.id);
        }
        let left = journal.load_all().unwrap();
        assert!(left.len() < written.len(), "nothing was rotated away");
        assert_eq!(left.last().unwrap().id, *written.last().unwrap());
        let size = std::fs::metadata(journal.path()).unwrap().len();
        let headroom = 400 * 1024;
        assert!(
            size <= 1024 * 1024 + headroom,
            "the journal outgrew the limit with no headroom: {size} bytes"
        );
    }

    #[test]
    fn one_entry_over_the_limit_is_kept_and_the_limit_yields() {
        // Retention drops the oldest entries, never the entry that was just written: a limit
        // smaller than one entry leaves that entry on disk, and the journal stays readable
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.max_entries = 0;
        config.journal.max_size_mb = 1;
        let mut journal = FileJournal::open_for(&config).unwrap();
        let mut entry = test_entry("2026-09-05T10:00:00Z", 4, 3.0, "kitty");
        entry.text_final = Some("x".repeat(2 * 1024 * 1024));
        journal.append(&entry).unwrap();
        let left = journal.load_all().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, entry.id);
    }

    #[test]
    fn a_missing_key_over_an_encrypted_journal_is_an_error_and_creates_no_key() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.encrypt = true;
        let key = directory.path().join("journal.key");
        {
            let mut journal = FileJournal::open_for(&config).unwrap();
            journal.append(&private_entry()).unwrap();
        }
        std::fs::remove_file(&key).unwrap();

        let error = FileJournal::open_for(&config).unwrap_err().to_string();
        assert!(error.contains("key"), "{error}");
        assert!(error.contains("encrypted"), "{error}");
        assert!(
            !key.exists(),
            "a new key was written over ciphertext: {error}"
        );
        let on_disk = std::fs::read_to_string(directory.path().join("journal.jsonl")).unwrap();
        assert!(
            on_disk.contains("enc1:"),
            "the fixture must hold ciphertext"
        );
    }

    #[test]
    fn a_damaged_line_also_blocks_a_new_key() {
        // A line that does not parse may hide ciphertext, so a new key is refused as well
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.encrypt = true;
        let key = directory.path().join("journal.key");
        let journal_path = directory.path().join("journal.jsonl");
        FileJournal::open(&journal_path).unwrap();
        let mut file = OpenOptions::new().append(true).open(&journal_path).unwrap();
        writeln!(file, "{{not json").unwrap();
        drop(file);

        assert!(FileJournal::open_for(&config).is_err());
        assert!(!key.exists());
    }

    #[test]
    fn a_missing_key_over_a_plain_journal_is_created() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        let path = directory.path().join("journal.jsonl");
        // A history written without encryption holds no ciphertext, so a new key hides nothing
        let mut plain = FileJournal::open(&path).unwrap();
        plain.append(&private_entry()).unwrap();
        drop(plain);

        config.journal.encrypt = true;
        assert!(FileJournal::open_for(&config).unwrap().encrypts());
        assert!(directory.path().join("journal.key").is_file());
    }

    #[test]
    fn concurrent_key_creation_leaves_one_key() {
        use std::sync::{Arc, Barrier};

        let directory = tempfile::tempdir().unwrap();
        let mut config = config_in(directory.path());
        config.journal.encrypt = true;
        let key = directory.path().join("journal.key");
        let config = Arc::new(config);
        let barrier = Arc::new(Barrier::new(6));
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let config = Arc::clone(&config);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    FileJournal::open_for(&config).unwrap()
                })
            })
            .collect();
        let journals: Vec<FileJournal> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        // Every process must see the same key, or the entries written by one are unreadable
        // for the others
        let mut writer = journals.into_iter().next().unwrap();
        let entry = private_entry();
        writer.append(&entry).unwrap();
        assert_eq!(writer.load_all().unwrap(), vec![entry]);
        assert_eq!(std::fs::read_to_string(&key).unwrap().trim().len(), 64);
    }
}
