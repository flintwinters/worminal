fn main() {
    let arguments: Vec<_> = std::env::args_os().collect();
    let status = match worminal_daemon::run(&arguments) {
        Ok(status) => status,
        Err(error) => {
            worminal_daemon::process::error(&error.to_string());
            worminal_daemon::process::error("Workspace service exited with status 1");
            eprintln!("{error}");
            1
        }
    };
    std::process::exit(status);
}
