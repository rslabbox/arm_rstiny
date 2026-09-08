fn main() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("link.lds");
    println!("cargo:rustc-link-arg-bin=hello=-T{}", script.display());
    println!("cargo:rerun-if-changed={}", script.display());
}
