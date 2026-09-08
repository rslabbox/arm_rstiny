fn main() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("link.lds");
    println!("cargo:rustc-link-arg-bin=fatboot=-T{}", script.display());
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=HELLO_ELF");
    let mut assembly = String::from(
        ".section .rodata.hello,\"a\"\n.balign 8\n.global __hello_start\n__hello_start:\n",
    );
    if let Some(path) = std::env::var_os("HELLO_ELF") {
        let path = std::fs::canonicalize(path).expect("hello ELF");
        let path = path.to_str().expect("UTF-8 hello ELF path");
        assert!(!path.contains(['\n', '\r', '"', '\\']));
        println!("cargo:rerun-if-changed={path}");
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        std::fs::read(path).expect("read hello ELF").hash(&mut hash);
        assembly.push_str(&format!("// payload {}\n", hash.finish()));
        assembly.push_str(&format!(".incbin \"{path}\"\n"));
    }
    assembly.push_str(".global __hello_end\n__hello_end:\n");
    // Empty resources permit workspace checks but cannot boot hello.
    std::fs::write(
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("hello.rs"),
        format!("core::arch::global_asm!({assembly:?});\n"),
    )
    .unwrap();
}
