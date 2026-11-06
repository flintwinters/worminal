use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/workspace/restore_format.h");
    let handoff = root.join("src/workspace/handoff_format.h");
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", handoff.display());
    bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .clang_arg("-include")
        .clang_arg(handoff.to_str().unwrap())
        .allowlist_var("RESTORE_.*|HANDOFF_.*")
        .allowlist_type("restore_limits")
        .prepend_enum_name(false)
        .generate()
        .expect("generate recovery format constants")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("restore.rs"))
        .expect("write recovery format constants");
}
