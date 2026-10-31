use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/terminal/engine.h");
    let archive = root.join("build/libworminal_engine.a");
    assert!(
        archive.is_file(),
        "Build the C engine through manage.py check-engine first"
    );
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", archive.display());
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("build").display()
    );
    println!("cargo:rustc-link-lib=static=worminal_engine");
    println!("cargo:rustc-link-lib=m");
    bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .allowlist_function("worminal_engine_.*")
        .allowlist_type("Worminal.*|worminal_.*|glyph_attribute|selection_.*|win_mode")
        .allowlist_var("(WORMINAL|ATTR|SEL|SNAP|MODE)_.*")
        .prepend_enum_name(false)
        .derive_default(true)
        .derive_partialeq(true)
        .generate_comments(true)
        .generate()
        .expect("generate engine bindings from engine.h")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("engine.rs"))
        .expect("write engine bindings");
}
