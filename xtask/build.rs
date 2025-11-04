use std::{env, fs, path::Path};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    // Read project name from the root Cargo.toml
    let project_root = Path::new(&manifest_dir).parent().unwrap();
    let project_cargo_toml = project_root.join("Cargo.toml");

    if let Ok(cargo_content) = fs::read_to_string(&project_cargo_toml) {
        // 简单解析 Cargo.toml 获取 package.name
        if let Some(name_line) = cargo_content
            .lines()
            .find(|line| line.trim().starts_with("name") && line.contains("="))
        {
            if let Some(name) = name_line.split('=').nth(1) {
                let project_name = name.trim().trim_matches('"').trim_matches('\'');
                println!("cargo:rustc-env=PROJECT_NAME={}", project_name);
            }
        }
    }
}

