fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let platform = if let Some(path) = std::env::var_os("PLATFORM_DIR") {
        std::path::PathBuf::from(path)
    } else {
        // Direct Cargo builds use the fixed EL1 QEMU platform too.
        let output = out.join("platform");
        let status = std::process::Command::new("python3")
            .arg(root.join("tools/build_platform.py"))
            .arg(&output)
            .arg("--qemu")
            .arg(std::env::var("QEMU").unwrap_or_else(|_| "qemu-system-aarch64".into()))
            .status()
            .expect("run platform generator");
        assert!(status.success(), "platform generation failed");
        output
    };
    let generated = platform.join("platform.rs");
    std::fs::copy(&generated, out.join("platform.rs")).expect("generated platform.rs missing");
    println!("cargo:rerun-if-changed={}", generated.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("tools/build_platform.py").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("kernel/plat/qemu-arm-virt/overlay.dts").display()
    );
    println!("cargo:rerun-if-env-changed=PLATFORM_DIR");
    println!("cargo:rerun-if-env-changed=QEMU");
    let level = std::env::var("LOG").unwrap_or_else(|_| "info".into());
    assert!(
        matches!(
            level.as_str(),
            "off" | "error" | "warn" | "info" | "debug" | "trace"
        ),
        "LOG must be off, error, warn, info, debug or trace"
    );
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../link.lds");
    println!("cargo:rustc-link-arg-bin=kernel=-T{}", script.display());
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=LOG");
}
