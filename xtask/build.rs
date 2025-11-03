use std::{env, fs, path::Path};

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let dst = Path::new(&out_dir).join("generated_mods.rs");
    let mut mods = String::new();

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let plugins_dir = Path::new(&manifest_dir).join("src/plugins");

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

    // Collect all plugin module names
    let mut plugin_names = Vec::new();

    let entries = std::fs::read_dir(&plugins_dir).unwrap();
    for entry in entries {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().map(|e| e == "rs").unwrap_or(false) {
            let mod_name = path.file_stem().unwrap().to_string_lossy().to_string();
            let abs_path = path.canonicalize().unwrap();
            let path_str = abs_path.to_string_lossy().replace('\\', "/");

            // Generate the module declaration with #[path] attribute
            mods.push_str(&format!(
                "#[path = \"{}\"]\npub mod {};\n",
                path_str, mod_name
            ));

            plugin_names.push(mod_name);
        }
    }

    // Generate the fetch_task function
    mods.push_str("\n");
    mods.push_str("use crate::utils::TaskResult;\n\n");
    mods.push_str("/// Auto-generated task fetching function\n");
    mods.push_str(
        "pub fn fetch_task(task: &str, args: &[String], config: &toml::Value) -> TaskResult<Box<dyn TaskPlugin>> {\n",
    );
    mods.push_str("    match task {\n");

    for plugin_name in &plugin_names {
        mods.push_str(&format!(
            "        \"{}\" => Ok(Box::new({}::TaskInstance::new(args, config))),\n",
            plugin_name, plugin_name
        ));
    }

    mods.push_str("        _ => Err(crate::utils::TaskError::TaskNotFound(task.to_string())),\n");
    mods.push_str("    }\n");
    mods.push_str("}\n");

    mods.push_str("\n");
    mods.push_str("/// Tasks List\n");
    mods.push_str("pub fn list_tasks() -> Vec<&'static str> {\n");
    mods.push_str("    vec![\n");
    for plugin_name in &plugin_names {
        mods.push_str(&format!("        \"{}\",\n", plugin_name));
    }
    mods.push_str("    ]\n");
    mods.push_str("}\n");

    mods.push_str("\n");
    mods.push_str("/// Tasks Descriptions\n");
    mods.push_str("pub fn task_descriptions(task: &str) -> &'static str {\n");
    mods.push_str("    match task {\n");
    for plugin_name in &plugin_names {
        mods.push_str(&format!(
            "        \"{}\" => {}::TaskInstance::description(),\n",
            plugin_name, plugin_name
        ));
    }
    mods.push_str("        _ => \"Unknown task\",\n");
    mods.push_str("    }\n");
    mods.push_str("}\n");

    fs::write(&dst, mods).unwrap();
    println!("cargo:rerun-if-changed=src/plugins");
}
