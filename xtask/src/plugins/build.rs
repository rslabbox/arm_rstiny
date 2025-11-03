use serde::Deserialize;
use std::collections::HashMap;
use crate::utils::{project_root, TaskResult};
use std::{env, path::PathBuf, process::Command};

#[derive(Debug, Default)]
struct BuildOptions {
    log: Option<String>,
    mode: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct BuildConfig {
    mode: String,
    target: String,
    log: String,
    tool_path: String,
    load_address: usize,
    entry_point: usize,
}

fn parse_build_options(args: &mut impl Iterator<Item = String>) -> TaskResult<BuildOptions> {
    let mut options = BuildOptions::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => {
                options.log = args.next();
            }
            "--mode" => {
                options.mode = args.next();
            }
            "--help" | "-h" => {
                println!("Build Task Help:");
                println!("  --log <level>    Set the log level (e.g., info, debug)");
                println!("  --mode <mode>    Set the build mode (e.g., debug, release)");
                return Err(crate::utils::TaskError::Ok);
            }
            _ => {
                return Err(crate::utils::TaskError::UnknownArgument(arg));
            }
        }
    }

    Ok(options)
}

pub struct BuildTask {
    project_name: String,
    build_config: BuildConfig,
}

impl BuildTask {
    pub fn elf_name(&self) -> PathBuf {
        project_root()
            .join("target")
            .join(self.build_config.target.clone())
            .join(self.build_config.mode.clone())
            .join(self.project_name.clone())
    }
}

impl super::TaskPlugin for BuildTask {
    fn new(args: &[String], config: &toml::Value) -> Self {
        let options = parse_build_options(&mut args.iter().cloned()).unwrap_or_default();
        let mut build_config: BuildConfig = config
            .get("build")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default();
        if let Some(log) = options.log {
            build_config.log = log;
        }
        if let Some(mode) = options.mode {
            build_config.mode = mode;
        }

        BuildTask {
            project_name: env!("PROJECT_NAME").to_string(),
            build_config,
        }
    }

    fn description() -> &'static str {
        "Build the project with specified configurations."
    }

    fn execute(&self) -> TaskResult<()> {
        let project_root = project_root();

        info!("==> Read configs:");
        info!("    Project Name: {}", self.project_name);
        info!("    Model: {}", self.build_config.mode);
        info!("    Target Platform: {}", self.build_config.target);
        info!("    Log Level: {}", self.build_config.log);
        info!("    Tool path: {}", self.build_config.tool_path);
        info!("    Output Dir: {}", self.elf_name().display());

        // Build cargo arguments
        let mut cargo_args = vec!["build".to_string()];
        let mut cargo_envs: HashMap<String, String> = HashMap::new();

        // Add mode argument
        if self.build_config.mode == "release" {
            cargo_args.push("--release".to_string());
        }

        // Add target platform
        cargo_args.push("--target".to_string());
        cargo_args.push(self.build_config.target.to_string());

        // Add linker lds
        let linker_path = "link.lds";
        cargo_envs.insert(
            "RUSTFLAGS".to_string(),
            format!("-C link-arg=-T{}", linker_path),
        );

        // Add log level
        cargo_envs.insert("LOG".to_string(), self.build_config.log.to_string());

        info!("==> Execute build command: cargo {}", cargo_args.join(" "));

        let cargo_status = Command::new("cargo")
            .args(&cargo_args)
            .current_dir(&project_root)
            .envs(&cargo_envs)
            .status()?;

        if !cargo_status.success() {
            return Err(crate::utils::TaskError::ExecutionFailed(
                "cargo build".into(),
            ));
        }

        info!("==> Build succeeded!");
        info!("    ELF file: {}", self.elf_name().display());

        info!("==> Copy binary");
        let objcopy_status = Command::new("rust-objcopy")
            .arg("-O")
            .arg("binary")
            .arg(self.elf_name().to_str().unwrap())
            .arg(self.elf_name().with_extension("bin").to_str().unwrap())
            .status()?;

        if !objcopy_status.success() {
            return Err(crate::utils::TaskError::ExecutionFailed(
                "rust-objcopy".into(),
            ));
        }
        info!(
            "    Binary file: {}",
            self.elf_name().with_extension("bin").display()
        );

        info!("==> Objdump");
        let objdump_output = Command::new("rust-objdump")
            .arg("-d")
            .arg("--print-imm-hex")
            .arg(self.elf_name().to_str().unwrap())
            .output()?;

        if !objdump_output.status.success() {
            return Err(crate::utils::TaskError::ExecutionFailed(
                "rust-objdump".into(),
            ));
        }

        // Write objdump output to .asm file
        let asm_file = self.elf_name().with_extension("asm");
        std::fs::write(&asm_file, objdump_output.stdout)?;
        info!("    ASM file: {}", asm_file.display());

        info!("==> Mkimage");
        let mkimage_status = Command::new("mkimage")
            .arg("-A")
            .arg("arm")
            .arg("-O")
            .arg("linux")
            .arg("-T")
            .arg("kernel")
            .arg("-C")
            .arg("none")
            .arg("-a")
            .arg(format!("0x{:x}", self.build_config.load_address))
            .arg("-e")
            .arg(format!("0x{:x}", self.build_config.entry_point))
            .arg("-n")
            .arg(&self.project_name)
            .arg("-d")
            .arg(self.elf_name().with_extension("bin").to_str().unwrap())
            .arg(self.elf_name().with_extension("uimg").to_str().unwrap())
            .status()?;
        if !mkimage_status.success() {
            return Err(crate::utils::TaskError::ExecutionFailed("mkimage".into()));
        }
        info!(
            "    UIMG file: {}",
            self.elf_name().with_extension("uimg").display()
        );

        Ok(())
    }
}

pub use BuildTask as TaskInstance;
