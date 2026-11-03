fn main() {
    if let Err(error) = worminal_frontend::app::run() {
        eprintln!("worminal-rust: {error}");
        std::process::exit(if error.kind() == std::io::ErrorKind::InvalidInput {
            2
        } else {
            1
        });
    }
}
