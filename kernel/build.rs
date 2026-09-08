fn main() {
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
