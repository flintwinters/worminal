//! Opening tabs: a new terminal beside the one being viewed, or one a bookmark names.
use super::App;
use std::{ffi::OsString, path::PathBuf, rc::Rc};
use worminal_protocol::worp::{
    commands::{CommandKind, OpenMode},
    launch::Launch,
};

impl App {
    pub(super) fn new_tab(self: &Rc<Self>) {
        // A new tab opens on the host of the tab being viewed, in that tab's directory.
        let directory = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.id == self.active.get())
            .map(|page| page.directory.clone())
            .filter(|directory| !directory.as_os_str().is_empty());
        self.open_terminal(self.active.get(), directory, Vec::new());
    }
    /// A new terminal beside tab `near`, on the daemon that owns it, in `directory` when given,
    /// running `command` (empty: the shell).
    pub(super) fn open_terminal(
        self: &Rc<Self>,
        near: u32,
        directory: Option<PathBuf>,
        command: Vec<OsString>,
    ) {
        match self.terminal_launch(directory, command) {
            Ok(launch) => {
                let link = &self.primary;
                link.pending_tabs
                    .set(link.pending_tabs.get().saturating_add(1));
                self.send(
                    near,
                    CommandKind::Open {
                        launch,
                        mode: OpenMode::Subscribe,
                    },
                );
            }
            Err(error) => self.message(Some(&error.to_string())),
        }
    }
    fn terminal_launch(
        &self,
        directory: Option<PathBuf>,
        command: Vec<OsString>,
    ) -> std::io::Result<Launch> {
        let mut config = self.config.clone();
        config.command = command;
        config.title = "Worminal".into();
        config.output.clear();
        config.line.clear();
        config.browser_url = None;
        if let Some(directory) = directory {
            config.cwd = directory;
        }
        Launch::decode(&config.launch()?)
    }
    /// A bookmarked terminal: a fresh shell in its directory on its host: this machine, a master already
    /// shown, or a master attached for it, as `--master` attaches one.
    pub(super) fn open_bookmarked_terminal(
        self: &Rc<Self>,
        directory: String,
        master: Option<String>,
    ) {
        let directory = PathBuf::from(directory);
        let Some(master) = master else {
            self.open_terminal(0, Some(directory), Vec::new());
            return;
        };
        let near = self
            .pages
            .borrow()
            .iter()
            .find(|page| page.host.as_ref() == Some(&master))
            .map(|page| page.id);
        match near {
            Some(near) => self.open_terminal(near, Some(directory), Vec::new()),
            None => match self.terminal_launch(Some(directory), Vec::new()) {
                Ok(launch) => self.attach_master(&OsString::from(master), Some(Box::new(launch))),
                Err(error) => self.message(Some(&error.to_string())),
            },
        }
    }
    /// A bookmarked server: a new tab on this machine that runs `ssh` to it and, when ssh ends, stays open in the
    /// user's shell, so a refused or dropped connection leaves its error on screen instead of closing the tab
    /// (a tab whose command exits closes at once). The endpoint is one validated argument, never part of the shell
    /// text. The script runs in an interactive `sh` for two reasons found by trying dash and bash:
    /// - job control gives ssh its own foreground process group, which is how the daemon recognises a tab that
    ///   is running ssh: it labels the tab with the user and stops it being bookmarked as a local directory;
    /// - a non-interactive shell exits when its foreground child dies of SIGINT, which would close the tab on
    ///   Ctrl+C during the connection. Bash does so even under `set -m` and a trap; an interactive one survives
    ///   with `trap : INT`. A trap, unlike ignoring the signal, is reset in ssh, so Ctrl+C still aborts it.
    pub(super) fn open_bookmarked_host(self: &Rc<Self>, endpoint: String, port: Option<u16>) {
        self.open_terminal(0, None, ssh_command(&endpoint, port));
    }
}

/// `sh -ic 'trap : INT; ssh ${2:+-p "$2"} "$1"; exec "$SHELL"' sh ENDPOINT PORT`. The port is the second
/// argument, empty when the bookmark has none, so ssh keeps its own default (22 or `~/.ssh/config`).
fn ssh_command(endpoint: &str, port: Option<u16>) -> Vec<OsString> {
    [
        "sh",
        "-ic",
        "trap : INT; ssh ${2:+-p \"$2\"} \"$1\"; exec \"${SHELL:-/bin/sh}\"",
        "sh",
        endpoint,
        &port.map(|port| port.to_string()).unwrap_or_default(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::ssh_command;

    #[test]
    fn the_endpoint_is_an_argument_of_the_script_never_part_of_it() {
        let command = ssh_command("me@build; reboot", Some(2222));
        assert_eq!(command.len(), 6);
        assert_eq!(command[4], "me@build; reboot");
        assert_eq!(command[5], "2222");
        let script = command[2].to_str().unwrap();
        assert!(
            !script.contains("build") && !script.contains("reboot") && !script.contains("2222")
        );
        assert!(script.contains("ssh ${2:+-p \"$2\"} \"$1\""));
        assert!(script.contains("trap : INT"));
        // No port leaves the second argument empty, so ssh gets no -p and keeps its own default.
        assert_eq!(ssh_command("h", None)[5], "");
    }
}
