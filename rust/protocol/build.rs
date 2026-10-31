use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/workspace/wire.h");
    println!("cargo:rerun-if-changed={}", header.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("src/workspace/service_policy.h").display()
    );
    bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg("-DWORMINAL_PREVIEW_PROTOCOL")
        .allowlist_type("wire_type|wire_feature")
        .allowlist_var("WIRE_.*|WORKSPACE_.*")
        .prepend_enum_name(false)
        .generate()
        .expect("generate protocol constants from wire.h")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("constants.rs"))
        .expect("write protocol constants");
}
