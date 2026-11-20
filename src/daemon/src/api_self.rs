//! `worminald api self`: which tab runs this process. The daemon's tab records already carry each
//! shell's pid, so identity is the caller's ancestor chain matched against `list`; nothing is injected
//! into the tab and no daemon state changes. Pids are local, so this never crosses `--master`.
use std::{collections::BTreeSet, fs, io};

pub const NOT_IN_A_TAB: &str = "This process is not running in a tab of this workspace.";

fn parent_of(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces or parentheses; the fields start after its last ')'.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

pub fn ancestors(mut pid: u32) -> BTreeSet<u32> {
    let mut chain = BTreeSet::new();
    while pid > 1 && chain.insert(pid) {
        let Some(parent) = parent_of(pid) else { break };
        pid = parent;
    }
    chain
}

/// Builds the reply from a `list` reply: the tab whose live shell is an ancestor of the caller.
pub fn identify(list_json: &str, chain: &BTreeSet<u32>) -> io::Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(list_json).map_err(|_| io::Error::other("Invalid tab list reply."))?;
    let tab = value["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| {
            tab["shell_pid"]
                .as_u64()
                .is_some_and(|pid| pid > 0 && chain.contains(&(pid as u32)))
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, NOT_IN_A_TAB))?;
    Ok(serde_json::json!({"ok": true, "api_version": 1, "id": tab["id"], "tab": tab}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"ok":true,"tabs":[{"id":1,"shell_pid":0},{"id":2,"shell_pid":50},{"id":3,"shell_pid":60}]}"#;

    #[test]
    fn picks_the_tab_whose_shell_is_an_ancestor_and_ignores_exited_shells() {
        let reply = identify(LIST, &BTreeSet::from([7, 60, 90])).unwrap();
        let value: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            (value["id"].as_u64(), value["tab"]["shell_pid"].as_u64()),
            (Some(3), Some(60))
        );
        assert!(identify(LIST, &BTreeSet::from([0, 7])).is_err());
    }

    #[test]
    fn own_chain_includes_this_process() {
        assert!(ancestors(std::process::id()).contains(&std::process::id()));
    }
}
