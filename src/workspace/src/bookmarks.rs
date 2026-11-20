//! Saved terminal tabs and web pages in the user's hand-editable `bookmarks.yaml`:
//!
//! ```yaml
//! bookmarks:
//!   - directory: /home/me/project     # a terminal: where its shell starts
//!     master: me@build-host           # optional: the SSH endpoint whose daemon owns that directory
//!     title: project                  # optional
//!   - url: https://example.org        # a web page
//! ```
//!
//! A bookmark never holds a command: like restart recovery, opening one starts a fresh shell. The file is
//! per user and shared by every window and workspace scope, so every change is a read-modify-publish under
//! a lock and a damaged file is reported and kept, never reset.
use crate::yamlfile::{self, FILE_LIMIT};
use serde_yaml_ng::{Mapping, Value};
use std::{io, path::PathBuf};
use worminal_protocol::worp::devices::validate_endpoint;

pub const MAX_BOOKMARKS: usize = 256;
const PATH_BYTES: usize = 4096;
const TITLE_BYTES: usize = 256;
const HEADER: &str = "# Worminal bookmarks: `directory: /abs/path` (optional `master: user@host`) or `url: ...`,\n\
# each with an optional `title`. Hand edits are fine; the app rewrites this file without comments.\n";

/// What a bookmark opens. Two bookmarks with the same target are the same bookmark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Terminal {
        directory: String,
        master: Option<String>,
    },
    Web {
        url: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub target: Target,
    pub title: Option<String>,
}

pub struct Bookmarks {
    path: PathBuf,
}

fn invalid(reason: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Invalid bookmarks file ({reason}); fix or remove the file, it is never reset."),
    )
}

fn rejected(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason.to_owned())
}

fn plain(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

impl Bookmark {
    pub fn validate(&self) -> io::Result<()> {
        if self
            .title
            .as_deref()
            .is_some_and(|title| !plain(title, TITLE_BYTES))
        {
            return Err(rejected(
                "Bookmark title must be 1-256 bytes without control characters.",
            ));
        }
        match &self.target {
            Target::Terminal { directory, master } => {
                if !directory.starts_with('/') || !plain(directory, PATH_BYTES) {
                    return Err(rejected(
                        "Bookmark directory must be an absolute path of up to 4096 bytes.",
                    ));
                }
                if let Some(master) = master {
                    validate_endpoint(master)?;
                }
            }
            Target::Web { url } => {
                if !plain(url, PATH_BYTES) || url.chars().any(char::is_whitespace) {
                    return Err(rejected(
                        "Bookmark URL must be 1-4096 bytes without spaces or control characters.",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Hand-written YAML reads `title: 2024` as a number; keep it as the text that was typed.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn decode_entry(entry: Value) -> io::Result<Bookmark> {
    let Value::Mapping(fields) = entry else {
        return Err(invalid("each bookmark is a mapping"));
    };
    let (mut directory, mut master, mut url, mut title) = (None, None, None, None);
    for (key, value) in fields {
        let key = key
            .as_str()
            .ok_or_else(|| invalid("keys are strings"))?
            .to_owned();
        let value = text(&value).ok_or_else(|| invalid(format!("`{key}` must be text")))?;
        let slot = match key.as_str() {
            "directory" => &mut directory,
            "master" => &mut master,
            "url" => &mut url,
            "title" => &mut title,
            _ => return Err(invalid(format!("unknown key `{key}`"))),
        };
        *slot = Some(value);
    }
    let target = match (directory, url, master) {
        (Some(directory), None, master) => Target::Terminal { directory, master },
        (None, Some(url), None) => Target::Web { url },
        (None, Some(_), Some(_)) => return Err(invalid("`master` belongs to a `directory`")),
        _ => {
            return Err(invalid(
                "each bookmark has exactly one of `directory` or `url`",
            ))
        }
    };
    let bookmark = Bookmark { target, title };
    bookmark
        .validate()
        .map_err(|error| invalid(format!("{error}")))?;
    Ok(bookmark)
}

fn decode(text: &str) -> io::Result<Vec<Bookmark>> {
    let root: Mapping = serde_yaml_ng::from_str::<Option<Mapping>>(text)
        .map_err(invalid)?
        .unwrap_or_default();
    let mut bookmarks: Vec<Bookmark> = Vec::new();
    for (key, value) in root {
        if key.as_str() != Some("bookmarks") {
            return Err(invalid("only a top-level `bookmarks` list is allowed"));
        }
        let entries = match value {
            Value::Null => Vec::new(),
            Value::Sequence(entries) => entries,
            _ => return Err(invalid("`bookmarks` must be a list")),
        };
        if entries.len() > MAX_BOOKMARKS {
            return Err(invalid(format!("more than {MAX_BOOKMARKS} bookmarks")));
        }
        for entry in entries {
            let bookmark = decode_entry(entry)?;
            if bookmarks.iter().any(|old| old.target == bookmark.target) {
                return Err(invalid("the same bookmark is listed twice"));
            }
            bookmarks.push(bookmark);
        }
    }
    Ok(bookmarks)
}

fn encode(bookmarks: &[Bookmark]) -> io::Result<Vec<u8>> {
    let entries: Vec<Value> = bookmarks
        .iter()
        .map(|bookmark| {
            let mut fields = Mapping::new();
            match &bookmark.target {
                Target::Terminal { directory, master } => {
                    fields.insert("directory".into(), directory.clone().into());
                    if let Some(master) = master {
                        fields.insert("master".into(), master.clone().into());
                    }
                }
                Target::Web { url } => {
                    fields.insert("url".into(), url.clone().into());
                }
            }
            if let Some(title) = &bookmark.title {
                fields.insert("title".into(), title.clone().into());
            }
            Value::Mapping(fields)
        })
        .collect();
    let mut root = Mapping::new();
    root.insert("bookmarks".into(), Value::Sequence(entries));
    let body = serde_yaml_ng::to_string(&root).map_err(io::Error::other)?;
    Ok(format!("{HEADER}{body}").into_bytes())
}

impl Bookmarks {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Every bookmark in the order the file lists them; a missing file is an empty list.
    pub fn list(&self) -> io::Result<Vec<Bookmark>> {
        match yamlfile::read(&self.path, &|reason| invalid(reason))? {
            Some(text) => decode(&text),
            None => Ok(Vec::new()),
        }
    }

    fn update(
        &self,
        change: impl FnOnce(&mut Vec<Bookmark>) -> io::Result<bool>,
    ) -> io::Result<bool> {
        yamlfile::update(
            &self.path,
            &|reason| invalid(reason),
            "Bookmarks are busy; try again.",
            || {
                let mut bookmarks = self.list()?;
                if !change(&mut bookmarks)? {
                    return Ok(None);
                }
                let bytes = encode(&bookmarks)?;
                if bytes.len() > FILE_LIMIT {
                    return Err(rejected("Bookmarks are full; remove one first."));
                }
                Ok(Some(bytes))
            },
        )
    }

    /// Add the bookmark, or remove the one with the same target. Returns whether it is bookmarked now.
    pub fn toggle(&self, bookmark: Bookmark) -> io::Result<bool> {
        bookmark.validate()?;
        let mut bookmarked = false;
        self.update(|bookmarks| {
            if let Some(index) = bookmarks
                .iter()
                .position(|old| old.target == bookmark.target)
            {
                bookmarks.remove(index);
            } else if bookmarks.len() >= MAX_BOOKMARKS {
                return Err(rejected("Bookmarks are full; remove one first."));
            } else {
                bookmarks.push(bookmark);
                bookmarked = true;
            }
            Ok(true)
        })?;
        Ok(bookmarked)
    }

    /// Remove the bookmark with this target; false when there was none.
    pub fn remove(&self, target: &Target) -> io::Result<bool> {
        self.update(|bookmarks| {
            let before = bookmarks.len();
            bookmarks.retain(|bookmark| &bookmark.target != target);
            Ok(before != bookmarks.len())
        })
    }
}
