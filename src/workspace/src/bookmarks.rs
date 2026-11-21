//! Saved terminal tabs and web pages in the user's hand-editable `bookmarks.yaml`:
//!
//! ```yaml
//! bookmarks:
//!   - directory: /home/me/project     # a terminal: where its shell starts
//!     master: me@build-host           # optional: the SSH endpoint whose daemon owns that directory
//!     title: project                  # optional
//!   - url: https://example.org        # a web page
//!   - host: me@build-host             # a server: opens a tab that runs ssh to it
//!     port: 2222                      # optional: ssh's own default (22, or ~/.ssh/config) when absent
//! ```
//!
//! A bookmark never holds a command: like restart recovery, opening one starts a fresh shell. A `host` is only a
//! validated SSH endpoint; the application builds the ssh invocation itself. The file is
//! per user and shared by every window and workspace scope, so every change is a read-modify-publish under
//! a lock and a damaged file is reported and kept, never reset.
use crate::yamlfile::{self, FILE_LIMIT};
use serde_yaml_ng::{Mapping, Value};
use std::{io, path::PathBuf};
use worminal_protocol::worp::devices::validate_endpoint;

pub const MAX_BOOKMARKS: usize = 256;
const DEFAULT_SSH_PORT: u16 = 22;
const PATH_BYTES: usize = 4096;
const TITLE_BYTES: usize = 256;
const HEADER: &str = "# Worminal bookmarks: `directory: /abs/path` (optional `master: user@host`), `url: ...` or\n\
# `host: user@host` (optional `port`), each with an optional `title`. Hand edits are fine; the app rewrites this file without comments.\n";

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
    /// A server reached with `ssh`: a host or `user@host`, on `port` (`None` is ssh's default, 22 unless
    /// `~/.ssh/config` says otherwise). The same host on another port is another bookmark.
    Host {
        endpoint: String,
        port: Option<u16>,
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

/// ssh's default port is the same bookmark as no port at all.
fn normal_port(port: Option<u16>) -> Option<u16> {
    port.filter(|port| *port != DEFAULT_SSH_PORT)
}

/// A typed server: `host`, `user@host`, any of them with `:port`, or `[v6]:port`. A bare IPv6 address has several
/// colons and takes no port. Port 22 is the default, so it reads as no port.
pub fn parse_server(text: &str) -> io::Result<(String, Option<u16>)> {
    let text = text.trim();
    let split = match text.rfind(':') {
        Some(at) if text[at + 1..].contains(']') => None,
        Some(at) if text[..at].ends_with(']') || text[..at].matches(':').count() == 0 => Some(at),
        _ => None,
    };
    let (endpoint, port) = match split {
        Some(at) => {
            let port = text[at + 1..]
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| rejected("The port after `:` is a number from 1 to 65535."))?;
            (&text[..at], Some(port))
        }
        None => (text, None),
    };
    validate_endpoint(endpoint)?;
    Ok((endpoint.to_owned(), normal_port(port)))
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
            Target::Host { endpoint, port } => {
                validate_endpoint(endpoint)?;
                if *port == Some(0) {
                    return Err(rejected("Bookmark port must be 1-65535."));
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
    let (mut directory, mut master, mut url, mut host, mut title, mut port) =
        (None, None, None, None, None, None);
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
            "host" => &mut host,
            "port" => &mut port,
            "title" => &mut title,
            _ => return Err(invalid(format!("unknown key `{key}`"))),
        };
        *slot = Some(value);
    }
    let port = match port {
        None => None,
        Some(_) if host.is_none() => return Err(invalid("`port` belongs to a `host`")),
        Some(port) => Some(
            port.parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid("`port` is a number from 1 to 65535"))?,
        ),
    };
    let target = match (directory, url, host, master) {
        (Some(directory), None, None, master) => Target::Terminal { directory, master },
        (None, Some(url), None, None) => Target::Web { url },
        (None, None, Some(endpoint), None) => Target::Host {
            endpoint,
            port: normal_port(port),
        },
        (None, _, _, Some(_)) => return Err(invalid("`master` belongs to a `directory`")),
        _ => {
            return Err(invalid(
                "each bookmark has exactly one of `directory`, `url` or `host`",
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
                Target::Host { endpoint, port } => {
                    fields.insert("host".into(), endpoint.clone().into());
                    if let Some(port) = port {
                        fields.insert("port".into(), u64::from(*port).into());
                    }
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

    /// Add the bookmark unless one with the same target exists. Returns whether it was added.
    pub fn add(&self, bookmark: Bookmark) -> io::Result<bool> {
        bookmark.validate()?;
        self.update(|bookmarks| {
            if bookmarks.iter().any(|old| old.target == bookmark.target) {
                return Ok(false);
            }
            if bookmarks.len() >= MAX_BOOKMARKS {
                return Err(rejected("Bookmarks are full; remove one first."));
            }
            bookmarks.push(bookmark);
            Ok(true)
        })
    }

    /// Name the bookmark with this target, or clear its name (`None`). Returns whether anything changed.
    pub fn rename(&self, target: &Target, title: Option<String>) -> io::Result<bool> {
        Bookmark {
            target: target.clone(),
            title: title.clone(),
        }
        .validate()?;
        self.update(|bookmarks| {
            let Some(bookmark) = bookmarks.iter_mut().find(|old| &old.target == target) else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "That bookmark no longer exists.",
                ));
            };
            if bookmark.title == title {
                return Ok(false);
            }
            bookmark.title = title;
            Ok(true)
        })
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
