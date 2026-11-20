use std::{env, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let header = root.join("src/terminal/engine.h");
    // Cross targets (the static musl daemon) link their own archive; tools/build.py engine_archive owns the path.
    let target = env::var("TARGET").unwrap();
    let directory = if target == env::var("HOST").unwrap() {
        root.join("build")
    } else {
        root.join("build").join(&target)
    };
    let archive = directory.join("libworminal_engine.a");
    assert!(
        archive.is_file(),
        "Build the C engine through manage.py check-engine first"
    );
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-changed={}", archive.display());
    println!("cargo:rustc-link-search=native={}", directory.display());
    println!("cargo:rustc-link-lib=static=worminal_engine");
    println!("cargo:rustc-link-lib=m");
    bindgen::Builder::default()
        .header(header.to_str().unwrap())
        .allowlist_function("worminal_(engine|selection)_.*")
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
