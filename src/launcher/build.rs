use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/gtk/rust_frontend_module.h");
    println!("cargo:rerun-if-changed={}", header.display());
    bindgen::Builder::default()
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .header(header.to_str().unwrap())
        .allowlist_type("Worminal.*Frontend.*Entry")
        .allowlist_var("WORMINAL_.*FRONTEND_.*")
        .generate()
        .expect("generate frontend module ABI")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("rust_frontend.rs"))
        .expect("write frontend module ABI");
}
