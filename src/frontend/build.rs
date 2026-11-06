//! Generate module/protocol bindings and detect the available GTK backends.
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(worminal_gdk_x11)");
    let targets = std::process::Command::new("pkg-config")
        .args(["--variable=targets", "gtk+-3.0"])
        .output()
        .expect("GTK backend configuration");
    assert!(targets.status.success(), "GTK backend configuration");
    if targets
        .stdout
        .split(u8::is_ascii_whitespace)
        .any(|target| target == b"x11")
    {
        println!("cargo:rustc-cfg=worminal_gdk_x11");
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let frontend = root.join("src/gtk/rust_frontend_module.h");
    println!("cargo:rerun-if-changed={}", frontend.display());
    bindgen::Builder::default()
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .header(frontend.to_str().unwrap())
        .allowlist_type("WorminalRustFrontendEntry")
        .allowlist_var("WORMINAL_RUST_FRONTEND_.*")
        .generate()
        .expect("frontend module bindings")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("frontend.rs"))
        .expect("write frontend module bindings");
    let state = root.join("src/gtk/window_state_protocol.h");
    println!("cargo:rerun-if-changed={}", state.display());
    bindgen::Builder::default()
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .header(state.to_str().unwrap())
        .allowlist_var("WORMINAL_STATE_.*")
        .generate()
        .expect("window-state protocol bindings")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("window_state.rs"))
        .expect("write window-state protocol");
    let version = root.join("src/version.h");
    println!("cargo:rerun-if-changed={}", version.display());
    let version = fs::read_to_string(version).unwrap();
    let version = version
        .lines()
        .find_map(|line| {
            line.strip_prefix("#define WORMINAL_VERSION \"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .expect("product version header");
    println!("cargo:rustc-env=WORMINAL_VERSION={version}");
}
