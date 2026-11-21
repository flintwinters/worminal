//! Where each window's panes were: the split tree, divider positions, which tabs sit in which pane, and the
//! window's size and position, kept so an accidentally closed or crashed frontend reopens as it was.
//!
//! The daemon owns tabs and never learns about splits, and tab ids change when a restarted daemon recovers its
//! tabs, so a saved layout names a tab by its position in the daemon's tab order (`0` is the first tab). Recovery
//! restores tabs in that order. A position that no longer exists is dropped and its empty pane collapses into
//! the sibling; tabs the layout never mentioned join the first pane. A damaged file is reported and kept, never
//! reset, and never read as a layout.
//!
//! The file is JSON, bounded, written atomically, scoped like the recovery file, and owned by the frontend
//! process that writes it: it lists that process's windows in the order they were opened.
use crate::{statefile, yamlfile};
use serde_json::{json, Map, Value};
use std::{
    io,
    path::{Path, PathBuf},
};
use worminal_protocol::WIRE_VERSION;

const FORMAT_VERSION: u64 = 1;
const MAX_WINDOWS: usize = 64;
const MAX_PANES: usize = 256;
const MAX_TABS: u32 = 4096;
const MAX_DEPTH: usize = 32;
/// Divider positions are thousandths of the split's length.
pub const RATIO_FULL: u16 = 1000;
pub const RATIO_HALF: u16 = 500;

/// One pane or one split of two parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Pane {
        /// Positions in the daemon's tab order.
        tabs: Vec<u32>,
        /// The pane's selected tab, one of `tabs`.
        selected: Option<u32>,
    },
    Split {
        /// Children sit left and right; otherwise top and bottom.
        side_by_side: bool,
        /// The first child's share, in thousandths.
        ratio: u16,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// One window: where it was and what its panes held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// `[x, y, width, height]`; the position is the window manager's to honor.
    pub geometry: [i32; 4],
    pub maximized: bool,
    pub tree: Node,
    /// The pane (a leaf, in reading order) that had keyboard focus.
    pub focused: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub windows: Vec<Window>,
}

fn invalid(reason: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Invalid saved window layout ({reason}); remove the file to start fresh, it is never reset."),
    )
}

/// The layout file for a workspace scope, beside that scope's recovery file.
pub fn path(state_home: &Path, scope: &[u8]) -> io::Result<PathBuf> {
    if !state_home.is_absolute() || scope.len() > 127 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid layout root or workspace scope",
        ));
    }
    let scope: String = scope.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(state_home
        .join("worminal")
        .join(format!("layout-v{WIRE_VERSION}-{scope}.json")))
}

impl Node {
    /// A pane holding these tabs, the first selected.
    pub fn pane(tabs: Vec<u32>) -> Self {
        let selected = tabs.first().copied();
        Self::Pane { tabs, selected }
    }

    /// The panes in reading order.
    pub fn panes(&self) -> Vec<&Node> {
        match self {
            Self::Pane { .. } => vec![self],
            Self::Split { first, second, .. } => {
                let mut panes = first.panes();
                panes.extend(second.panes());
                panes
            }
        }
    }

    /// This tree for a workspace with `count` tabs: unknown and repeated positions are dropped, panes left
    /// empty collapse into their sibling, and tabs the tree never held join the first pane. The result always
    /// has at least one pane, and every tab is in exactly one.
    pub fn fit(self, count: u32) -> Self {
        let mut seen = vec![false; count as usize];
        let kept = self.keep(&mut seen);
        let mut tree = kept.unwrap_or(Self::Pane {
            tabs: Vec::new(),
            selected: None,
        });
        let missing: Vec<u32> = (0..count).filter(|tab| !seen[*tab as usize]).collect();
        if !missing.is_empty() {
            tree.first_pane().extend(missing);
        }
        tree.first_selection();
        tree
    }

    fn keep(self, seen: &mut [bool]) -> Option<Self> {
        match self {
            Self::Pane { tabs, selected } => {
                let mut kept = Vec::new();
                for tab in tabs {
                    // Unknown positions and ones an earlier pane already holds are dropped.
                    if let Some(taken) = seen.get_mut(tab as usize).filter(|taken| !**taken) {
                        *taken = true;
                        kept.push(tab);
                    }
                }
                let tabs = kept;
                if tabs.is_empty() {
                    return None;
                }
                let selected = selected.filter(|tab| tabs.contains(tab));
                Some(Self::Pane { tabs, selected })
            }
            Self::Split {
                side_by_side,
                ratio,
                first,
                second,
            } => match (first.keep(seen), second.keep(seen)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    side_by_side,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (only, None) | (None, only) => only,
            },
        }
    }

    fn first_pane(&mut self) -> &mut Vec<u32> {
        match self {
            Self::Pane { tabs, .. } => tabs,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    /// A pane whose selection was lost shows its first tab.
    fn first_selection(&mut self) {
        match self {
            Self::Pane { tabs, selected } => {
                if selected.is_none_or(|tab| !tabs.contains(&tab)) {
                    *selected = tabs.first().copied();
                }
            }
            Self::Split { first, second, .. } => {
                first.first_selection();
                second.first_selection();
            }
        }
    }

    fn encode(&self) -> Value {
        match self {
            Self::Pane { tabs, selected } => json!({"tabs": tabs, "selected": selected}),
            Self::Split {
                side_by_side,
                ratio,
                first,
                second,
            } => json!({
                "split": if *side_by_side { "side_by_side" } else { "stacked" },
                "ratio": ratio,
                "first": first.encode(),
                "second": second.encode(),
            }),
        }
    }

    fn decode(value: &Value, depth: usize, panes: &mut usize) -> io::Result<Self> {
        let fields = value.as_object().ok_or_else(|| invalid("a node is a mapping"))?;
        if depth > MAX_DEPTH {
            return Err(invalid("splits nest too deeply"));
        }
        if let Some(kind) = fields.get("split") {
            let side_by_side = match kind.as_str() {
                Some("side_by_side") => true,
                Some("stacked") => false,
                _ => return Err(invalid("a split is `side_by_side` or `stacked`")),
            };
            let ratio = number(fields, "ratio")?;
            if !(1..u32::from(RATIO_FULL)).contains(&ratio) {
                return Err(invalid("a divider ratio is between 1 and 999"));
            }
            let mut child = |name: &str| {
                Self::decode(
                    fields.get(name).ok_or_else(|| invalid("a split has two parts"))?,
                    depth + 1,
                    panes,
                )
            };
            let first = child("first")?;
            let second = child("second")?;
            return Ok(Self::Split {
                side_by_side,
                ratio: ratio as u16,
                first: Box::new(first),
                second: Box::new(second),
            });
        }
        *panes += 1;
        if *panes > MAX_PANES {
            return Err(invalid("too many panes"));
        }
        let tabs = fields
            .get("tabs")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("a pane lists its tabs"))?
            .iter()
            .map(|tab| position(tab))
            .collect::<io::Result<Vec<u32>>>()?;
        let selected = match fields.get("selected") {
            None | Some(Value::Null) => None,
            Some(tab) => Some(position(tab)?),
        };
        Ok(Self::Pane { tabs, selected })
    }
}

fn number(fields: &Map<String, Value>, name: &str) -> io::Result<u32> {
    fields
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid(format!("`{name}` is a number")))
}

fn position(value: &Value) -> io::Result<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value < MAX_TABS)
        .ok_or_else(|| invalid("a tab position is a small number"))
}

impl Saved {
    pub fn encode(&self) -> Vec<u8> {
        let windows: Vec<Value> = self
            .windows
            .iter()
            .map(|window| {
                json!({
                    "geometry": window.geometry,
                    "maximized": window.maximized,
                    "focused": window.focused,
                    "tree": window.tree.encode(),
                })
            })
            .collect();
        let document = json!({"version": FORMAT_VERSION, "windows": windows});
        let mut bytes = serde_json::to_vec_pretty(&document).unwrap_or_default();
        bytes.push(b'\n');
        bytes
    }

    pub fn decode(text: &str) -> io::Result<Self> {
        let document: Value = serde_json::from_str(text).map_err(invalid)?;
        let fields = document
            .as_object()
            .ok_or_else(|| invalid("the file is a mapping"))?;
        if fields.get("version").and_then(Value::as_u64) != Some(FORMAT_VERSION) {
            return Err(invalid("unknown version"));
        }
        let entries = fields
            .get("windows")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("`windows` is a list"))?;
        if entries.len() > MAX_WINDOWS {
            return Err(invalid("too many windows"));
        }
        let mut windows = Vec::new();
        for entry in entries {
            let entry = entry.as_object().ok_or_else(|| invalid("a window is a mapping"))?;
            let numbers = entry
                .get("geometry")
                .and_then(Value::as_array)
                .filter(|numbers| numbers.len() == 4)
                .ok_or_else(|| invalid("`geometry` is four numbers"))?;
            let mut geometry = [0i32; 4];
            for (slot, value) in geometry.iter_mut().zip(numbers) {
                *slot = value
                    .as_i64()
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| invalid("`geometry` is four numbers"))?;
            }
            if geometry[2] < 1 || geometry[3] < 1 {
                return Err(invalid("a window has a size"));
            }
            let mut panes = 0;
            let tree = Node::decode(
                entry.get("tree").ok_or_else(|| invalid("a window has a `tree`"))?,
                0,
                &mut panes,
            )?;
            windows.push(Window {
                geometry,
                maximized: entry.get("maximized").and_then(Value::as_bool).unwrap_or(false),
                focused: entry
                    .get("focused")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(0),
                tree,
            });
        }
        Ok(Self { windows })
    }
}

/// The saved layout, or `None` when there is none. A file that cannot be read as one is an error and stays.
pub fn load(path: &Path) -> io::Result<Option<Saved>> {
    match yamlfile::read(path, &|reason| invalid(reason))? {
        Some(text) => Saved::decode(&text).map(Some),
        None => Ok(None),
    }
}

/// Replace the saved layout. A damaged file is left alone: only `forget` removes it.
pub fn store(path: &Path, saved: &Saved) -> io::Result<()> {
    load(path)?;
    statefile::atomic_write(path, &saved.encode())
}

/// Delete the saved layout so the next start is fresh.
pub fn forget(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
