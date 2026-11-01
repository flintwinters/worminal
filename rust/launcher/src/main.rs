use worminal_launcher::application;
fn main() -> std::process::ExitCode {
    application::main(application::Frontend::Legacy, false)
}
