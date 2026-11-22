//! CLI spellings are declared once: parsers, native help and search share these definitions.
use std::ffi::OsStr;

pub struct Entry {
    pub names: &'static [&'static str],
    pub arguments: &'static str,
    pub summary: &'static str,
}

pub struct Group {
    pub help: &'static str,
    pub prefix: &'static str,
    pub entries: &'static [Entry],
}

impl Group {
    pub fn reference(&self) -> String {
        self.entries
            .iter()
            .map(|entry| {
                format!(
                    "  {} {} {}  {}\n",
                    self.prefix,
                    entry.names.join(" | "),
                    entry.arguments,
                    entry.summary
                )
            })
            .collect()
    }
}

macro_rules! commands {
    ($($group:ident($help:literal, $prefix:literal) {
        $($variant:ident [$($name:literal),+] $arguments:literal => $summary:literal;)*
    })*) => {
        $(
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub enum $group { $($variant,)* }
            impl $group {
                pub const ALL: &'static [Self] = &[$(Self::$variant,)*];
                pub const GROUP: Group = Group {
                    help: $help, prefix: $prefix,
                    entries: &[$(Entry { names: &[$($name),+], arguments: $arguments, summary: $summary },)*],
                };
                pub fn parse(value: &str) -> Option<Self> {
                    match value { $($($name)|+ => Some(Self::$variant),)* _ => None }
                }
                pub fn matches(self, value: impl AsRef<OsStr>) -> bool {
                    value.as_ref().to_str().and_then(Self::parse) == Some(self)
                }
                pub fn entry(self) -> &'static Entry {
                    &Self::GROUP.entries[self as usize]
                }
                pub fn name(self) -> &'static str { self.entry().names[0] }
            }
        )*
        pub const GROUPS: &[Group] = &[$($group::GROUP,)*];
    };
}

commands! {
    Launcher("worminal --help", "worminal") {
        Help ["--help", "-h"] "" => "Show the launcher reference without starting a GUI.";
        Version ["--version", "-v"] "" => "Print the installed version.";
        Attach ["--attach"] "" => "Attach to existing tabs without opening a shell.";
        Alternate ["-a"] "" => "Disable the alternate screen.";
        Fixed ["-i"] "" => "Use fixed window geometry.";
        Execute ["-e", "--"] "PROGRAM ARG..." => "Run a program; remaining arguments are literal.";
        Browser ["--surf", "--browser"] "URL|FILE" => "Open a browser window.";
        Master ["--master"] "HOST" => "Join this SSH master's tabs in the active window.";
        Class ["-c"] "CLASS" => "Set the window class.";
        Name ["-n"] "NAME" => "Set the window instance name.";
        Title ["-T", "-t"] "TITLE" => "Set the window title.";
        Font ["-f"] "FONT" => "Override the terminal font.";
        Geometry ["-g"] "COLSxROWS" => "Set terminal columns and rows.";
        Output ["-o"] "FILE|-" => "Set printer output; - transfers the caller's stdout.";
        Line ["-l"] "LINE" => "Set the terminal line setting.";
        DetachMaster ["--detach-master"] "HOST" => "Detach this master connection; remote shells keep running.";
        CloseMaster ["--close-master"] "HOST" => "Close this master connection's tabs; terminates their programs.";
        Bookmark ["--bookmark"] "" => "Bookmark the directory of the tab this command runs in.";
        NewBrowser ["--new-browser-tab"] "URL|FILE" => "Replace the calling terminal tab with a browser.";
        ReplaceOwner ["--replace-owner"] "TOKEN" => "Internal GUI ownership receiver; not a manual update trigger.";
    }
    Daemon("worminald --help", "worminald") {
        Help ["--help", "-h"] "" => "Show native command discovery without starting a daemon.";
        Version ["--version"] "" => "Print the installed version.";
        Api ["api"] "COMMAND" => "Headless workspace commands; use api help for reference topics.";
        Attach ["attach"] "[--master HOST] [--existing]" => "Frontend Worp session on stdin/stdout; see attach --help.";
        Config ["config"] "[OPTIONS]|sync SSH_ENDPOINT" => "Inspect, validate or synchronize runtime configuration.";
        InstallRemote ["install-on-remote"] "SSH_ENDPOINT" => "Bootstrap a new headless node over SSH.";
        UpgradeRemote ["upgrade-remote"] "SSH_ENDPOINT" => "Sync an older node with a newer peer.";
        ListMasters ["--list-masters"] "" => "List current and previous master connections as JSON.";
        Serve ["--serve"] "" => "Run the persistent workspace service.";
        Bridge ["--bridge"] "" => "Bridge stdin/stdout to a workspace, starting it if absent.";
        BridgeExisting ["--bridge-existing"] "" => "Bridge to an existing workspace without starting it.";
        Stop ["--stop"] "" => "Stop the workspace service and its programs.";
        Handin ["--handin"] "NAME" => "Prepare to receive transferred live terminal ownership.";
        Handoff ["--handoff"] "NAME" => "Transfer live terminal ownership to a prepared receiver.";
        NewBrowser ["--new-browser-tab"] "URL|FILE" => "Replace the calling terminal tab with a browser.";
        UpgradeReceiver ["--upgrade-receiver"] "" => "Internal automatic daemon upgrade receiver.";
        HandoffReceiver ["--handoff-receiver"] "" => "Internal daemon ownership transfer receiver.";
        SshChild ["--ssh-child"] "OWNER_PID PROGRAM ARG..." => "Internal lifetime-guarded SSH child.";
        ConnectMaster ["--connect-master"] "HOST BRIDGE_MODE" =>
            "Internal authenticated master connection bootstrap.";
    }
    Api("worminald api help", "worminald api") {
        Help ["help", "--help", "-h"] "[TOPIC]|search QUERY..." => "Show references or fuzzy-search all CLI commands and options.";
        Session ["--session"] "" => "Duplex binary Worp session to the existing daemon.";
        Stdio ["--stdio"] "" => "Internal single-request binary Worp transport.";
        List ["list"] "" => "List tab IDs, titles, directories and shell PIDs as JSON.";
        Doctor ["doctor"] "--json" => "Diagnose scope, endpoint, versions and shell markers.";
        SelfTab ["self"] "" => "Identify the caller's ancestor terminal tab; local only.";
        Open ["open"] "[OPTIONS] [-- PROGRAM ARG...]" => "Create a terminal or browser tab.";
        Close ["close"] "TAB" => "Close a terminal/browser tab and hang up its programs.";
        Read ["read"] "TAB... [OPTIONS]" => "Passively read viewport, history or command output.";
        Wait ["wait"] "TAB [OPTIONS]" => "Wait for changed observations or explicit command completion.";
        Type ["type"] "TAB [OPTIONS] TEXT|--stdin" => "Queue literal text or paste without an implicit Enter.";
        Key ["key"] "TAB [--guard TOKEN] KEY..." => "Queue named application keys, navigation and modifiers.";
        Devices ["devices"] "[--no-check]" => "List enrollment and optionally check reachability.";
        Enroll ["enroll"] "NAME ENDPOINT" => "Enroll a named SSH route without contacting it.";
        Forget ["forget"] "NAME" => "Remove enrollment without stopping the device or its sessions.";
    }
    ApiOption("worminald api help", "") {
        Master ["--master"] "user@host" => "Target another node over SSH; precedes the API command.";
        Guard ["--guard"] "TOKEN" => "Validate the foreground identity before type/key input.";
        Then ["--then"] "KEY" => "Follow type's paste atomically with a named key.";
        Stdin ["--stdin"] "" => "Read bounded UTF-8 paste from standard input for type.";
        Tail ["--tail"] "N" => "Read the last 1..1000 retained physical rows.";
        Since ["--since"] "CURSOR" => "Read newly retained output after an opaque cursor.";
        Command ["--command"] "N" => "Read command output or wait for explicit completion.";
        Workspace ["--workspace"] "WORKSPACE" => "Bind a command wait to its observed workspace.";
        After ["--after"] "REVISION" => "Wait for a changed observation.";
        Timeout ["--timeout"] "SECONDS" => "Bound a wait to 0..60 seconds.";
        Cwd ["--cwd"] "/ABS/PATH" => "Open a terminal in this absolute directory.";
        Title ["--title"] "TITLE" => "Set a new terminal's title.";
        Columns ["--columns"] "N" => "Set a new terminal's width (default 80).";
        Rows ["--rows"] "N" => "Set a new terminal's height (default 24).";
        Browser ["--browser"] "URL" => "Open a browser record using an explicit URL.";
        NoCheck ["--no-check"] "" => "List device enrollment without network probes.";
        Json ["--json"] "" => "Print structured doctor diagnostics.";
        End ["--"] "" => "End options before literal text or a program's arguments.";
    }
    AttachOption("worminald attach --help", "worminald attach") {
        Master ["--master"] "HOST" => "Attach to this SSH node after version and configuration sync.";
        Existing ["--existing"] "" => "Never start a daemon; fail when none is running.";
    }
    Config("worminald config --help", "worminald config") {
        Sync ["sync"] "[--prefer local|remote] SSH_ENDPOINT" => "Merge configuration per key with an SSH peer.";
        Exchange ["exchange"] "" => "Internal configuration exchange over standard input/output.";
    }
    ConfigOption("worminald config --help", "worminald config") {
        Help ["--help", "-h"] "" => "Show configuration commands, files and settings.";
        Json ["--json"] "[--defaults]" => "Print resolved settings as JSON.";
        Defaults ["--defaults"] "" => "With --json, ignore configuration files.";
        Paths ["--paths"] "" => "Show configuration file paths.";
        Example ["--example"] "" => "Print a complete runnable configuration example.";
        Prefer ["--prefer"] "local|remote" => "Resolve config sync conflicts using the selected side.";
    }
    Topic("worminald api help", "worminald api help") {
        Keys ["keys"] "" => "Named keys, modifiers and target terminal modes.";
        Observe ["observe"] "" => "Scrollback, batch reads, revisions, wait and guarded input.";
        ShellBash ["shell-bash"] "" => "Optional runnable Bash integration; eval in interactive .bashrc.";
        ShellZsh ["shell-zsh"] "" => "Optional runnable Zsh integration; eval in interactive .zshrc.";
        Schema ["schema"] "" => "JSON reply shapes, key names, errors and exit codes.";
        Session ["session"] "" => "Binary duplex session: framing, commands and events.";
        Search ["search"] "QUERY..." => "Fuzzy search across every registered CLI command and option.";
    }
}

/// Names the other executable for a top-level spelling tried on the wrong one, e.g.
/// `worminald --detach-master`; the two command sets overlap only in help/version/browser.
pub fn misplaced(value: &str, executable: &str) -> Option<String> {
    let [here, there] = if executable == Launcher::GROUP.prefix {
        [Launcher::GROUP, Daemon::GROUP]
    } else {
        [Daemon::GROUP, Launcher::GROUP]
    };
    let owns = |group: &Group| {
        group
            .entries
            .iter()
            .find(|entry| entry.names.contains(&value))
    };
    if owns(&here).is_some() {
        return None;
    }
    let entry = owns(&there)?;
    Some(format!(
        "{value} belongs to {prefix}: {prefix} {value} {arguments}  ({summary})",
        prefix = there.prefix,
        arguments = entry.arguments,
        summary = entry.summary,
    ))
}

pub fn documents() -> Vec<(String, String)> {
    GROUPS
        .iter()
        .map(|group| (group.help.to_owned(), group.reference()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_spelling_is_automatically_documented() {
        let documents = documents();
        for group in GROUPS {
            let reference = group.reference();
            assert!(documents
                .iter()
                .any(|(help, text)| help == group.help && text == &reference));
            for entry in group.entries {
                for name in entry.names {
                    assert!(reference.contains(name), "{} {name}", group.help);
                }
            }
        }
        assert!(misplaced("--detach-master", "worminald")
            .unwrap()
            .starts_with("--detach-master belongs to worminal: worminal --detach-master HOST"));
        assert!(misplaced("--list-masters", "worminal")
            .unwrap()
            .starts_with("--list-masters belongs to worminald"));
        assert_eq!(misplaced("--help", "worminald"), None);
        assert_eq!(misplaced("--bogus", "worminal"), None);
        for command in Launcher::ALL {
            for name in command.entry().names {
                assert_eq!(Launcher::parse(name), Some(*command));
            }
        }
    }
}
