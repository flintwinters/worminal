use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/gtk/frontend_module.h");
    println!("cargo:rerun-if-changed={}", header.display());
    bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .allowlist_type("WorminalFrontendEntry")
        .allowlist_var("WORMINAL_FRONTEND_.*")
        .generate()
        .expect("generate frontend module ABI")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("frontend.rs"))
        .expect("write frontend module ABI");
}
