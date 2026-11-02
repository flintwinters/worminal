use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    for (header, output) in [("rust_frontend_module.h", "rust_frontend.rs")] {
        let header = root.join("src/gtk").join(header);
        println!("cargo:rerun-if-changed={}", header.display());
        bindgen::Builder::default()
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            .header(header.to_str().unwrap())
            .allowlist_type("Worminal.*FrontendEntry")
            .allowlist_var("WORMINAL_.*FRONTEND_.*")
            .generate()
            .expect("generate frontend module ABI")
            .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(output))
            .expect("write frontend module ABI");
    }
}
