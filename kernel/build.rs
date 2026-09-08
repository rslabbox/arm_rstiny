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
    println!("cargo:rerun-if-env-changed=ROOT_IMAGE");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let image = match std::env::var_os("ROOT_IMAGE") {
        Some(path) => {
            let path = std::path::PathBuf::from(path)
                .canonicalize()
                .expect("ROOT_IMAGE not found; run make build");
            println!("cargo:rerun-if-changed={}", path.display());
            std::fs::read(path).expect("cannot read ROOT_IMAGE")
        }
        // Cargo check/clippy can run independently of the image pipeline.
        None => Vec::new(),
    };
    std::fs::write(out.join("root.boot"), image).unwrap();
}
