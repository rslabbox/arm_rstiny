use std::{env, fs, path::PathBuf};

fn main() {
    for key in ["BOOT_ARCHIVE", "PLATFORM_DIR", "QEMU"] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let archive = env::var_os("BOOT_ARCHIVE").map(|path| {
        PathBuf::from(path)
            .canonicalize()
            .expect("read boot archive")
    });
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = root.parent().unwrap();
    let generator = root.join("tools/build_platform.py");
    println!("cargo:rerun-if-changed={}", generator.display());
    let platform = if let Some(path) = env::var_os("PLATFORM_DIR") {
        PathBuf::from(path)
    } else {
        let directory = out.join("platform");
        let status = std::process::Command::new("python3")
            .arg(generator)
            .arg(&directory)
            .arg("--qemu")
            .arg(env::var("QEMU").unwrap_or_else(|_| "qemu-system-aarch64".into()))
            .status()
            .expect("run platform generator");
        assert!(status.success(), "platform generation failed");
        directory
    }
    .join("platform.rs");
    if let Some(archive) = &archive {
        println!("cargo:rerun-if-changed={}", archive.display());
    }
    println!("cargo:rerun-if-changed={}", platform.display());
    println!("cargo:rerun-if-changed=linker.ld");
    fs::copy(platform, out.join("platform.rs")).expect("copy platform configuration");
    // The archive is a linker section, not a Rust byte array. The source file
    // contains only the assembly directive needed to include an external file.
    let asm = if let Some(archive) = archive {
        let path = archive.to_str().expect("UTF-8 archive path");
        assert!(
            !path.contains(['\n', '\r', '"', '\\']),
            "unsupported archive path"
        );
        // Include payload identity in Rust input: incremental compilation cannot
        // otherwise see a changed file referenced only by an assembler incbin.
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        fs::read(&archive)
            .expect("read archive contents")
            .hash(&mut hash);
        format!(
            "// payload {}\n.section .boot_archive,\"a\"\n.balign 4\n.incbin \"{path}\"\n",
            hash.finish()
        )
    } else {
        // Workspace check/clippy need no image. An empty archive fails closed
        // at runtime; only the image builder supplies the bootable payload.
        ".section .boot_archive,\"a\"\n".to_owned()
    };
    fs::write(
        out.join("archive.rs"),
        format!("core::arch::global_asm!({asm:?});\n"),
    )
    .unwrap();
    let linker = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("linker.ld");
    println!("cargo:rustc-link-arg=-T{}", linker.display());
    println!("cargo:rustc-link-arg=--build-id=none");
}
