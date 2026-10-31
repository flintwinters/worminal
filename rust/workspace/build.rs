use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/workspace/restore_format.h");
    println!("cargo:rerun-if-changed={}", header.display());
    bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .allowlist_var("RESTORE_.*")
        .allowlist_type("restore_limits")
        .prepend_enum_name(false)
        .generate()
        .expect("generate recovery format constants")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("restore.rs"))
        .expect("write recovery format constants");
}
