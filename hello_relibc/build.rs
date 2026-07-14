use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest_dir.parent().expect("workspace root");
    let relibc_out = root
        .join("components")
        .join("athbridge")
        .join("relibc")
        .join("target")
        .join("x86_64-unknown-none")
        .join("release");

    println!("cargo:rustc-link-search=native={}", relibc_out.display());
    println!("cargo:rustc-link-lib=static=relibc");
    println!("cargo:rustc-link-lib=static=crt0");
    println!(
        "cargo:rerun-if-changed={}",
        relibc_out.join("librelibc.a").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        relibc_out.join("libcrt0.a").display()
    );
}
