// SPDX-License-Identifier: MIT
//! Paths of the Files tab: `~` expansion, the autocomplete of the input and the media kind
//!
//! The shell is not in the way here: a path typed into the window never reaches a shell, so `~`
//! must be expanded before the CLI is started. The listing itself is an ordinary `read_dir`.
//! A directory that cannot be read gives an empty list, never an error: the user keeps typing.

use std::path::{Path, PathBuf};

/// One entry of the path autocomplete
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PathSuggestion {
    /// What goes into the input: absolute or `~`-relative, a directory ends with a separator
    pub path: String,
    /// The entry name alone, for the list
    pub name: String,
    pub is_dir: bool,
}

/// The kind of media the window may show for a result
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Audio,
    Video,
}

/// Containers that usually carry a picture
///
/// The extension decides, because the name is all the GUI has before the file loads. A video
/// container that turns out to hold audio alone still plays, with an empty frame.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "ogv", "mpg", "mpeg", "wmv", "flv", "3gp", "ts",
];

/// The kind of a result: a container from the video list, everything else as audio
pub fn media_kind(path: &Path) -> MediaKind {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension {
        Some(extension) if VIDEO_EXTENSIONS.contains(&extension.as_str()) => MediaKind::Video,
        _ => MediaKind::Audio,
    }
}

/// The home directory, when the platform names one
pub fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// `~` and `~/…` against `home`; every other path stays as it was typed
///
/// `~name` is not a shell shortcut for a home directory and is left alone, so a file with that
/// name is still reachable.
pub fn expand_tilde_with(input: &str, home: Option<&Path>) -> PathBuf {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('~');
    if let (Some(rest), Some(home)) = (rest, home) {
        if rest.is_empty() {
            return home.to_path_buf();
        }
        if let Some(rest) = rest.strip_prefix('/') {
            return if rest.is_empty() {
                home.to_path_buf()
            } else {
                home.join(rest)
            };
        }
    }
    PathBuf::from(trimmed)
}

/// `expand_tilde_with` against the real home directory
pub fn expand_tilde(input: &str) -> PathBuf {
    let home = home_dir();
    expand_tilde_with(input, home.as_deref())
}

/// Whether a character separates path components on this platform
fn is_separator(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

/// Where the typed text splits: the directory to read and the part of a name to match
///
/// A trailing separator means "list this directory"; otherwise the text after the last
/// separator is the prefix. Text without a separator is not a path we can complete.
fn split_for_listing(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    // A bare `~` lists the home directory, not its parent with the user name as a prefix
    if trimmed == "~" {
        return Some(("~/".to_string(), String::new()));
    }
    if !(trimmed.starts_with('/') || trimmed.starts_with('~')) {
        return None;
    }
    match trimmed.rfind(is_separator) {
        Some(index) => {
            let (directory, name) = trimmed.split_at(index + 1);
            Some((directory.to_string(), name.to_string()))
        }
        None => None,
    }
}

/// The entries of the typed directory whose name starts with the typed prefix
///
/// Directories come first, then files, both by name. A directory suggestion ends with a
/// separator, so the next listing opens it instead of naming it.
pub fn suggest_paths_with(input: &str, home: Option<&Path>, limit: usize) -> Vec<PathSuggestion> {
    let Some((display_directory, prefix)) = split_for_listing(input) else {
        return Vec::new();
    };
    let directory = expand_tilde_with(&display_directory, home);
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let prefix_lower = prefix.to_lowercase();
    let mut found: Vec<(bool, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden entries appear only when the user asked for them
        if name.starts_with('.') && !prefix.starts_with('.') {
            continue;
        }
        if !prefix.is_empty() && !name.to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        let path = entry.path();
        // A symbolic link to a directory is a directory for navigation
        let is_dir = path.is_dir();
        if !is_dir && !path.is_file() {
            continue;
        }
        found.push((is_dir, name, path));
    }
    found.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let limit = limit.clamp(1, 50);
    found
        .into_iter()
        .take(limit)
        .map(|(is_dir, name, _)| {
            let mut path = format!("{display_directory}{name}");
            if is_dir {
                path.push('/');
            }
            PathSuggestion { path, name, is_dir }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("a temporary directory");
        std::fs::create_dir_all(root.path().join("Music")).expect("Music");
        std::fs::create_dir_all(root.path().join("Movies")).expect("Movies");
        std::fs::write(root.path().join("notes.txt"), b"x").expect("a file");
        std::fs::write(root.path().join(".secret"), b"x").expect("a hidden file");
        root
    }

    #[test]
    fn tilde_expands_only_for_the_current_user() {
        let home = Path::new("/home/ilya");
        assert_eq!(
            expand_tilde_with("~", Some(home)),
            PathBuf::from("/home/ilya")
        );
        assert_eq!(
            expand_tilde_with("~/Documents/a.wav", Some(home)),
            PathBuf::from("/home/ilya/Documents/a.wav")
        );
        // A file whose name starts with a tilde is not a home directory
        assert_eq!(
            expand_tilde_with("~ilya/notes.txt", Some(home)),
            PathBuf::from("~ilya/notes.txt")
        );
        assert_eq!(
            expand_tilde_with("/tmp/a.wav", Some(home)),
            PathBuf::from("/tmp/a.wav")
        );
    }

    #[test]
    fn an_empty_input_lists_nothing() {
        let home = Path::new("/home/ilya");
        assert!(suggest_paths_with("", Some(home), 10).is_empty());
        assert!(suggest_paths_with("   ", Some(home), 10).is_empty());
    }

    #[test]
    fn a_relative_path_is_not_completed() {
        // Only `~` and absolute paths name a directory the window can read
        assert!(suggest_paths_with("relative/dir/", None, 10).is_empty());
        assert!(suggest_paths_with("notes", None, 10).is_empty());
    }

    #[test]
    fn directories_come_first_and_end_with_a_separator() {
        let root = temp_tree();
        let input = format!("{}/", root.path().display());
        let found = suggest_paths_with(&input, None, 10);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Movies", "Music", "notes.txt"]);
        assert!(found[0].is_dir);
        assert!(found[0].path.ends_with("Movies/"));
        assert!(!found[2].is_dir);
        assert!(found[2].path.ends_with("notes.txt"));
    }

    #[test]
    fn the_typed_prefix_filters_case_insensitively() {
        let root = temp_tree();
        let input = format!("{}/m", root.path().display());
        let found = suggest_paths_with(&input, None, 10);
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Movies", "Music"]);
    }

    #[test]
    fn hidden_entries_need_a_dot_in_the_prefix() {
        let root = temp_tree();
        let all = suggest_paths_with(&format!("{}/", root.path().display()), None, 10);
        assert!(!all.iter().any(|s| s.name == ".secret"));
        let hidden = suggest_paths_with(&format!("{}/.", root.path().display()), None, 10);
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].name, ".secret");
    }

    #[test]
    fn a_directory_that_does_not_exist_is_an_empty_list() {
        let found = suggest_paths_with("/no/such/directory/", None, 10);
        assert!(found.is_empty());
    }

    #[test]
    fn the_limit_is_respected_and_bounded() {
        let root = temp_tree();
        let input = format!("{}/", root.path().display());
        assert_eq!(suggest_paths_with(&input, None, 2).len(), 2);
        // Zero is raised to one rather than returning everything or nothing
        assert_eq!(suggest_paths_with(&input, None, 0).len(), 1);
    }

    #[test]
    fn a_bare_tilde_lists_the_home_directory() {
        let home = tempfile::tempdir().expect("a home directory");
        std::fs::create_dir_all(home.path().join("Videos")).expect("Videos");
        let found = suggest_paths_with("~", Some(home.path()), 10);
        assert_eq!(found.len(), 1);
        // The suggestion keeps the form the user typed, so the input stays readable
        assert_eq!(found[0].path, "~/Videos/");
    }

    #[test]
    fn video_containers_are_told_from_audio() {
        assert_eq!(media_kind(Path::new("a/lecture.MP4")), MediaKind::Video);
        assert_eq!(media_kind(Path::new("a/interview.mkv")), MediaKind::Video);
        assert_eq!(media_kind(Path::new("a/voice.wav")), MediaKind::Audio);
        assert_eq!(media_kind(Path::new("a/voice.m4a")), MediaKind::Audio);
        assert_eq!(media_kind(Path::new("a/no-extension")), MediaKind::Audio);
    }
}
