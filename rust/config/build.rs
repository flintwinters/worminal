use std::{env, fs, path::PathBuf};

fn main() {
    let theme =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../../build/theme.rs");
    println!("cargo:rerun-if-changed={}", theme.display());
    fs::copy(
        theme,
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("theme.rs"),
    )
    .expect("generate theme through manage.py build first");
}
