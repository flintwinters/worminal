fn main() {
    if let Err(error) = worminal_frontend::app::run() {
        eprintln!("worminal-rust: {error}");
        std::process::exit(1);
    }
}
