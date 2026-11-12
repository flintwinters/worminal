//! Three-way merge of `worminal.yaml` between two nodes, per leaf key.
//!
//! Each side keeps a base: the last agreed content for a peer. A key changed on one side takes that side's
//! value; the same change on both is agreed; different changes are conflicts that stay unchanged on both
//! sides (and in the base) until the values meet or a side is preferred. Line-based merging would conflict
//! on unrelated neighbouring keys, so the unit is the dotted key path. Lists are single values.
use crate::source::{config_home, state_home};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub type Flat = BTreeMap<Vec<String>, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    Neither,
    Local,
    Remote,
}

#[derive(Debug, Default, PartialEq)]
pub struct Merged {
    pub local: Flat,
    pub remote: Flat,
    pub base: Flat,
    pub conflicts: Vec<String>,
}

fn walk(prefix: &mut Vec<String>, value: &Value, out: &mut Flat) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                prefix.push(key.clone());
                walk(prefix, value, out);
                prefix.pop();
            }
        }
        _ => {
            out.insert(prefix.clone(), value.clone());
        }
    }
}

pub fn flatten(value: &Value) -> Flat {
    let mut out = Flat::new();
    walk(&mut Vec::new(), value, &mut out);
    out
}

pub fn unflatten(flat: &Flat) -> Value {
    let mut root = Value::Object(Map::new());
    for (path, value) in flat {
        let mut node = &mut root;
        for key in path {
            node = node
                .as_object_mut()
                .expect("flattened paths never extend a leaf")
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Map::new()));
        }
        *node = value.clone();
    }
    root
}

pub fn merge3(base: &Flat, local: &Flat, remote: &Flat, prefer: Prefer) -> Merged {
    let mut merged = Merged::default();
    let keys: std::collections::BTreeSet<_> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .collect();
    for key in keys {
        let (b, l, r) = (base.get(key), local.get(key), remote.get(key));
        let chosen = if l == r || r == b {
            Some(l)
        } else if l == b {
            Some(r)
        } else {
            match prefer {
                Prefer::Local => Some(l),
                Prefer::Remote => Some(r),
                Prefer::Neither => None,
            }
        };
        let sides = match chosen {
            Some(value) => [value; 3],
            None => {
                merged.conflicts.push(key.join("."));
                [l, r, b]
            }
        };
        for (flat, value) in [&mut merged.local, &mut merged.remote, &mut merged.base]
            .into_iter()
            .zip(sides)
        {
            if let Some(value) = value {
                flat.insert(key.clone(), value.clone());
            }
        }
    }
    merged
}

/// Parse `worminal.yaml` text. Empty text is an empty configuration.
pub fn parse(text: &str) -> Result<Flat, String> {
    let value: Value = serde_yaml_ng::from_str(text).map_err(|error| error.to_string())?;
    match value {
        Value::Null => Ok(Flat::new()),
        Value::Object(_) => Ok(flatten(&value)),
        _ => Err("worminal.yaml must be a mapping".into()),
    }
}

/// Canonical text for a merged side. Comments and layout are not preserved.
pub fn render(flat: &Flat) -> String {
    if flat.is_empty() {
        return String::new();
    }
    serde_yaml_ng::to_string(&unflatten(flat)).expect("a JSON value serializes as YAML")
}

/// Where this node's `worminal.yaml` lives, whether or not it exists yet.
pub fn target_path() -> PathBuf {
    std::env::var_os("WORMINAL_CONFIG")
        .filter(|value| !value.is_empty())
        .map_or_else(
            || config_home().join("worminal/worminal.yaml"),
            PathBuf::from,
        )
}

/// Last agreed content with one peer, kept beside daemon state rather than in the user's config.
pub fn base_path(peer: &str) -> PathBuf {
    let name: String = peer
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "@._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    state_home()
        .join("worminal/config-sync")
        .join(format!("{name}.yaml"))
}

pub fn read_text(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Replace a file atomically so a reader never sees half a configuration.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("{}: {error}", path.display());
    fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(fail)?;
    let temporary = path.with_extension("yaml.pending");
    let mut file = fs::File::create(&temporary).map_err(fail)?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(fail)?;
    fs::rename(&temporary, path).map_err(fail)
}
