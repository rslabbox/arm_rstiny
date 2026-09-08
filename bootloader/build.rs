use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=KERNEL_LOAD_MIN");
    let minimum = env::var("KERNEL_LOAD_MIN").unwrap_or_else(|_| "0".into());
    println!("cargo:rustc-env=KERNEL_LOAD_MIN={minimum}");
    println!("cargo:rerun-if-env-changed=BOOT_ARCHIVE_OBJECT");
    if env::var("TARGET").unwrap() == env::var("HOST").unwrap() {
        return; // Host parser/planner tests neither embed an archive nor boot.
    }
    let archive = env::var_os("BOOT_ARCHIVE_OBJECT").map(|path| {
        PathBuf::from(path)
            .canonicalize()
            .expect("read boot archive object")
    });
    if let Some(archive) = &archive {
        println!("cargo:rerun-if-changed={}", archive.display());
    }
    println!("cargo:rerun-if-changed=linker.ld");
    // The image builder supplies an ordinary AArch64 relocatable object.
    // Cargo tracks its contents via rerun-if-changed and relinks when it changes.
    // Check/clippy need no object; the linker rejects an absent payload on build.
    if let Some(archive) = archive {
        println!("cargo:rustc-link-arg-bin=bootloader={}", archive.display());
    }
    let linker = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("linker.ld");
    println!("cargo:rustc-link-arg=-T{}", linker.display());
    println!("cargo:rustc-link-arg=--build-id=none");
}
