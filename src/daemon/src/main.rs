fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    use worminal_workspace::cli_schema::Daemon;
    let serving = matches!(
        arguments
            .get(1)
            .and_then(|arg| arg.to_str())
            .and_then(Daemon::parse),
        Some(Daemon::Serve | Daemon::UpgradeReceiver | Daemon::HandoffReceiver)
    );
    let status = match worminal_daemon::run(&arguments) {
        Ok(status) => status,
        Err(error) => {
            if serving {
                worminal_daemon::process::error(&error.to_string());
                worminal_daemon::process::error("Workspace service exited with status 1");
            }
            eprintln!("{}", worminal_workspace::cli_output::error(&error));
            1
        }
    };
    std::process::exit(status);
}
