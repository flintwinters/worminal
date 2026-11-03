fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    let serving = arguments.get(1).is_some_and(|arg| {
        arg == "--serve" || arg == "--upgrade-receiver" || arg == "--handoff-receiver"
    });
    let status = match worminal_daemon::run(&arguments) {
        Ok(status) => status,
        Err(error) => {
            if serving {
                worminal_daemon::process::error(&error.to_string());
                worminal_daemon::process::error("Workspace service exited with status 1");
            }
            eprintln!("{error}");
            1
        }
    };
    std::process::exit(status);
}
