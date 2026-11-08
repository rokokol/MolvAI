// SPDX-License-Identifier: MIT
//! Build script of the CLI: it turns the documents this repository ships into the table that
//! `search_docs` reads.
//!
//! The table is generated from the files instead of being written by hand, so the binary
//! carries the same pages a reader opens: a new file in `docs/` or `site/` enters the index by
//! existing, and no list has to be kept in step with them
//!
//! The documents live outside the package directory. A source tree assembled without them (a
//! packaged build, or the Nix expression that supplies `crates/` alone) gets an empty table
//! rather than a broken build, and the tool then reports an empty index. `MOLVA_DOCS_DIR`
//! names another root for the documents, which lets a packaging expression add them without
//! rehashing the sources

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process;

/// The site as GitHub Pages serves it from the repository today
const SITE_URL: &str = "https://rokokol.github.io/MolvAI/";

/// A markdown file has no published page of its own, so its URL opens it in the repository
const BLOB_URL: &str = "https://github.com/rokokol/MolvAI/blob/main/";

/// Elements whose text is navigation, code or decoration rather than the page's prose
const SKIPPED_ELEMENTS: &[&str] = &[
    "head", "script", "style", "svg", "template", "noscript", "iframe", "nav", "header", "footer",
    "aside",
];

/// One page of the index, in one language
struct Page {
    path: String,
    title: String,
    lang: &'static str,
    url: String,
    text: String,
}

fn main() {
    let root = docs_root();
    println!("cargo:rerun-if-env-changed=MOLVA_DOCS_DIR");
    // The directories are watched too: a page that appears or disappears must rebuild the table
    for name in ["README.md", "docs", "site"] {
        watch(&root.join(name));
    }

    let mut files = Vec::new();
    let readme = root.join("README.md");
    if readme.is_file() {
        files.push(readme);
    }
    files.extend(files_with_extension(&root.join("docs"), "md"));
    files.extend(files_with_extension(&root.join("site"), "html"));

    let mut pages = Vec::new();
    for file in &files {
        watch(file);
        match file.extension().and_then(|extension| extension.to_str()) {
            Some("md") => {
                if let Some(page) = markdown_page(&root, file) {
                    pages.push(page);
                }
            }
            Some("html") => pages.extend(site_pages(&root, file)),
            _ => {}
        }
    }
    write_table(&pages);
}

/// Ask cargo to rerun this script when the path changes
fn watch(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// The root of the documents: the repository unless the build was told otherwise
fn docs_root() -> PathBuf {
    if let Some(dir) = env::var_os("MOLVA_DOCS_DIR") {
        return PathBuf::from(dir);
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    manifest.join("..").join("..")
}

/// Files with this extension, sorted, recursively for directories
fn files_with_extension(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            files.extend(files_with_extension(&path, extension));
        } else if path.extension().and_then(|name| name.to_str()) == Some(extension) {
            files.push(path);
        }
    }
    files
}

/// The path inside the repository, with forward slashes on every system
fn relative(root: &Path, file: &Path) -> Option<String> {
    let rest = file.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rest
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    Some(parts.join("/"))
}

fn markdown_page(root: &Path, file: &Path) -> Option<Page> {
    let source = fs::read_to_string(file).ok()?;
    let path = relative(root, file)?;
    let title = source
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&path)
        .to_string();
    Some(Page {
        url: format!("{BLOB_URL}{path}"),
        text: markdown_text(&source),
        title,
        lang: "en",
        path,
    })
}

/// The markdown source as searchable prose: link targets, markup and tags go, the words stay
fn markdown_text(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut in_fence = false;
    for line in source.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        out.push_str(&plain_markup(line));
        out.push(' ');
    }
    collapse(&out)
}

/// One line of markdown with its markup removed; text inside a code fence passes through here
/// too, because a command name is worth searching for
fn plain_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut index = 0;
    while index < line.len() {
        let rest = &line[index..];
        if let Some(consumed) = skip_link(rest, &mut out) {
            index += consumed;
            continue;
        }
        if let Some(consumed) = skip_tag(rest) {
            index += consumed;
            continue;
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        if !matches!(ch, '*' | '`' | '#' | '>' | '|') {
            out.push(ch);
        }
        index += ch.len_utf8();
    }
    out
}

/// A link or an image at the start of the text, and how much of it was consumed. A link keeps
/// its text and loses its target; an image is a badge or a screenshot, so it goes whole. The
/// closing bracket is the matching one: an image inside a link would otherwise end the link at
/// its own alt text
fn skip_link(text: &str, out: &mut String) -> Option<usize> {
    let opening = usize::from(text.starts_with("!["));
    if !text[opening..].starts_with('[') {
        return None;
    }
    let close = matching_bracket(&text[opening..])?;
    let label = &text[opening + 1..opening + close];
    let tail = text[opening + close + 1..].strip_prefix('(')?;
    let end = tail.find(')')?;
    if opening == 0 {
        out.push_str(&plain_markup(label));
    }
    Some(opening + close + 1 + 1 + end + 1)
}

/// The index of the `]` that matches the `[` at the start of the text
fn matching_bracket(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, ch) in text.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// An HTML tag at the start of the text, if there is one; a stray `<` is prose, not a tag
fn skip_tag(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('<')?;
    let end = rest.find('>')?;
    if end > 200 || rest[..end].contains('<') {
        return None;
    }
    let name = rest[..end].trim_start_matches('/');
    let first = name.chars().next()?;
    if first.is_alphanumeric() || first == '!' {
        Some(end + 2)
    } else {
        None
    }
}

/// The page as two plain texts, English and Russian; the site carries the second one in the
/// `data-ru` attributes and the first one in the elements themselves
fn html_text(source: &str) -> (String, String) {
    let mut english = String::new();
    let mut russian = String::new();
    let mut skipped: Vec<&str> = Vec::new();
    let mut rest = source;
    while let Some(position) = rest.find('<') {
        if skipped.is_empty() {
            push_text(&mut english, &rest[..position]);
        }
        rest = &rest[position..];
        if rest.starts_with("<!--") {
            match rest.find("-->") {
                Some(end) => rest = &rest[end + 3..],
                None => break,
            }
            continue;
        }
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..end];
        rest = &rest[end + 1..];
        let closing = tag.starts_with('/');
        let name = tag_name(tag);
        let self_closing = tag.trim_end().ends_with('/');
        let skip_here =
            !closing && !self_closing && (SKIPPED_ELEMENTS.contains(&name) || is_skip_link(tag));
        if skipped.is_empty() && !closing && !skip_here {
            for value in ru_attributes(tag) {
                push_text(&mut russian, &plain_markup(&unescape(value)));
            }
        }
        if closing {
            if skipped.last() == Some(&name) {
                skipped.pop();
            }
        } else if skip_here {
            skipped.push(name);
        }
    }
    if skipped.is_empty() {
        push_text(&mut english, rest);
    }
    (collapse(&english), collapse(&russian))
}

fn tag_name(tag: &str) -> &str {
    let tag = tag.trim_start_matches('/');
    let end = tag
        .find(|ch: char| ch.is_whitespace() || ch == '/' || ch == '>')
        .unwrap_or(tag.len());
    &tag[..end]
}

/// The skip link repeats on every page and its text names the page's own table of contents
fn is_skip_link(tag: &str) -> bool {
    attribute(tag, "class").is_some_and(|class| class.split_whitespace().any(|name| name == "skip"))
}

/// Every `data-ru*` attribute of one tag: the Russian text of the element
fn ru_attributes(tag: &str) -> Vec<&str> {
    const MARKER: &str = "data-ru";
    let mut values = Vec::new();
    let mut rest = tag;
    while let Some(position) = rest.find(MARKER) {
        let after = &rest[position + MARKER.len()..];
        let name_end = after
            .find(|ch: char| ch == '=' || ch.is_whitespace())
            .unwrap_or(after.len());
        let name = &rest[position..position + MARKER.len() + name_end];
        if let Some(value) = attribute(tag, name) {
            values.push(value);
        }
        rest = after;
    }
    values
}

/// The value of one attribute of a tag, unquoted
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    loop {
        let position = rest.find(name)?;
        let boundary = position == 0 || rest[..position].ends_with(|ch: char| ch.is_whitespace());
        let after = &rest[position + name.len()..];
        if boundary {
            if let Some(value) = after.trim_start().strip_prefix('=') {
                let value = value.trim_start();
                let quote = value.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let body = &value[quote.len_utf8()..];
                    if let Some(end) = body.find(quote) {
                        return Some(&body[..end]);
                    }
                }
            }
        }
        rest = after;
    }
}

/// The contents of the first `<name>` element and its attributes
fn element<'a>(source: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{name}");
    let start = source.find(&open)?;
    let after = &source[start + open.len()..];
    let end = after.find('>')?;
    let inner = &after[end + 1..];
    let content = inner.find(&format!("</{name}>"))?;
    Some((&after[..end], &inner[..content]))
}

/// The page as two pages of the index, one per language the site translates
fn site_pages(root: &Path, file: &Path) -> Vec<Page> {
    let Ok(source) = fs::read_to_string(file) else {
        return Vec::new();
    };
    // A page the site keeps out of search engines stays out of this index as well
    if source.contains("name=\"robots\"") && source.contains("noindex") {
        return Vec::new();
    }
    let Some(path) = relative(root, file) else {
        return Vec::new();
    };
    let url = format!("{SITE_URL}{}", path.strip_prefix("site/").unwrap_or(&path));
    let (english, russian) = html_text(&source);
    let title = element(&source, "title");
    let english_title = title
        .map(|(_, inner)| collapse(&plain_markup(&unescape(inner))))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| path.clone());
    let russian_title = title
        .and_then(|(attributes, _)| attribute(attributes, "data-ru"))
        .map(|value| collapse(&plain_markup(&unescape(value))))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| english_title.clone());

    let mut pages = Vec::new();
    if !english.is_empty() {
        pages.push(Page {
            path: path.clone(),
            title: english_title,
            lang: "en",
            url: url.clone(),
            text: english,
        });
    }
    if !russian.is_empty() {
        pages.push(Page {
            title: russian_title,
            lang: "ru",
            text: russian,
            path,
            url,
        });
    }
    pages
}

/// Characters that attach to the text before them without a space: closing punctuation and the
/// hyphen or slash of a compound
const ATTACHING_START: &[char] = &[
    ',', '.', ';', ':', '!', '?', ')', ']', '}', '»', '…', '-', '/', '\'', '’',
];

/// Characters that attach to the text after them without a space
const ATTACHING_END: &[char] = &['(', '[', '{', '«', '“', '/', '-', '—'];

/// Append one text node. The site writes inline markup without whitespace around it, so
/// `<code>x</code>, y` must not become `x , y`; two blocks that touch without whitespace
/// between them still need the space that the markup removed
fn push_text(out: &mut String, text: &str) {
    let text = unescape(text);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let separator = text.starts_with(char::is_whitespace)
        || !(trimmed.starts_with(ATTACHING_START) || out.ends_with(ATTACHING_END));
    if separator && !out.is_empty() {
        out.push(' ');
    }
    out.push_str(trimmed);
}

/// Named and numeric entities as the characters they stand for; an unknown one stays as it is
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('&') {
        out.push_str(&rest[..position]);
        rest = &rest[position..];
        let entity = rest
            .find(';')
            .filter(|end| *end <= 12)
            .and_then(|end| entity(&rest[1..end]).map(|ch| (ch, end + 1)));
        if let Some((ch, consumed)) = entity {
            out.push(ch);
            rest = &rest[consumed..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        return char::from_u32(code);
    }
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "hellip" => Some('…'),
        "times" => Some('×'),
        "rarr" => Some('→'),
        "larr" => Some('←'),
        "copy" => Some('©'),
        "laquo" => Some('«'),
        "raquo" => Some('»'),
        _ => None,
    }
}

/// Single spaces between words, no leading or trailing space
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// The generated table, as `include!` reads it
fn write_table(pages: &[Page]) {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    let target = out_dir.join("docs_index.rs");
    let mut out = String::new();
    out.push_str("// Generated by build.rs from the repository documents: do not edit\n");
    out.push_str("const DOCS: &[Doc] = &[\n");
    for page in pages {
        out.push_str("    Doc {\n");
        push_field(&mut out, "path", &page.path);
        push_field(&mut out, "title", &page.title);
        push_field(&mut out, "lang", page.lang);
        push_field(&mut out, "url", &page.url);
        push_field(&mut out, "text", &page.text);
        out.push_str("    },\n");
    }
    out.push_str("];\n");
    if let Err(error) = fs::write(&target, out) {
        fail(&format!("cannot write {}: {error}", target.display()));
    }
}

fn push_field(out: &mut String, name: &str, value: &str) {
    out.push_str("        ");
    out.push_str(name);
    out.push_str(": ");
    push_literal(out, value);
    out.push_str(",\n");
}

fn push_literal(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if u32::from(ch) < 0x20 || ch == '\u{7f}' => {
                out.push_str(&format!("\\u{{{:x}}}", u32::from(ch)));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn fail(message: &str) -> ! {
    eprintln!("build.rs: {message}");
    process::exit(1);
}
