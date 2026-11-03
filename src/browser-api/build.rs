use std::{env, path::PathBuf};
fn main() {
    let header = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../src/gtk/rust_browser_module.h");
    println!("cargo:rerun-if-changed={}", header.display());
    bindgen::Builder::default()
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .header(header.to_str().unwrap())
        .allowlist_type("Worminal.*")
        .allowlist_var("WORMINAL_.*")
        .prepend_enum_name(false)
        .generate()
        .expect("generate browser ABI")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("browser.rs"))
        .expect("write browser ABI");
}
