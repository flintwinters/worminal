//! Native launcher reference shared with the headless daemon's search.
use crate::{cli_output, cli_schema::Launcher};

pub fn launcher(product: &str, version: &str) -> String {
    format!(
        "{product} {version}\n\nusage: {product} [OPTIONS] [PROGRAM ARG...]\n\n{}\n\
        Master commands use the exact SSH endpoint and target this display/workspace.\n\n\
        Ctrl+Shift+T: tab  Ctrl+Shift+N: window  Ctrl+Shift+W: close tab\n\
        Ctrl+Shift+C/V: clipboard  Ctrl+Shift+A: select all history\n\
        Ctrl+Tab / Ctrl+Shift+Tab or Ctrl+scroll: cycle tabs  Alt+1..9: select tab\n\
        Ctrl+= / Ctrl+-: zoom in/out  Ctrl+Shift+Home: reset zoom\n\
        Drag a tab to a pane edge: split  Ctrl+Shift+X: gather panes\n\
        Ctrl+Shift+Arrows: focus pane  Alt+Shift+Arrows: move tab to pane\n\
        Windows reopen with their layout after a crash or close; Ctrl+Shift+Q: close and forget it\n\
        Ctrl+Shift+D: bookmark tab (or its star)  Ctrl+Shift+B: side panel (`a` adds a server, `r` names)\n\
        Middle click: autoscroll  Shift+middle click: paste selection\n\n\
        Automation and agents: run `worminald --help`, then `worminald api help`\n\n{}",
        Launcher::GROUP
            .reference()
            .replace("worminal ", &format!("{product} ")),
        cli_output::HELP
    )
}
