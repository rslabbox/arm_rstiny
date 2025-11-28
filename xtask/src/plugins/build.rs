use crate::utils::{project_root, TaskResult};
use serde::Deserialize;
use std::path::PathBuf;
use xshell::{cmd, Shell};

#[derive(Debug, Deserialize, Default)]
struct BuildConfig {
    mode: String,
    target: String,
    log: String,
    tool_path: String,
    load_address: usize,
    entry_point: usize,
    #[serde(default)]
    features: Option<Vec<String>>,
    #[serde(default = "default_smp")]
    smp: u16,
}

fn default_smp() -> u16 {
    1
}

pub struct BuildTask {
    project_name: String,
    build_config: BuildConfig,
    features: Option<Vec<String>>,
}

impl BuildTask {
    pub fn new(options: crate::BuildOptions, config: &toml::Value) -> TaskResult<Self> {
        let mut build_config: BuildConfig = config
            .get("build")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default();
        
        // 用命令行参数覆盖配置文件
        if let Some(log) = options.log {
            build_config.log = log;
        }
        if let Some(mode) = options.mode {
            build_config.mode = mode;
        }

        // 处理 features 参数
        let features = if let Some(features_str) = options.features {
            // 命令行参数优先，解析逗号分隔的字符串
            Some(
                features_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            )
        } else {
            // 使用配置文件中的 features
            build_config.features.clone()
        };

        Ok(BuildTask {
            project_name: env!("PROJECT_NAME").to_string(),
            build_config,
            features,
        })
    }

    pub fn smp(&self) -> u16 {
        self.build_config.smp
    }

    pub fn elf_name(&self) -> PathBuf {
        project_root()
            .join("target")
            .join(self.build_config.target.clone())
            .join(self.build_config.mode.clone())
            .join(self.project_name.clone())
    }

    pub fn execute(&self) -> TaskResult<()> {
        let sh = Shell::new()?;
        let project_root = project_root();
        sh.change_dir(&project_root);

        info!("==> Read configs:");
        info!("    Project Name: {}", self.project_name);
        info!("    Mode: {}", self.build_config.mode);
        info!("    Target Platform: {}", self.build_config.target);
        info!("    Log Level: {}", self.build_config.log);
        info!("    Tool path: {}", self.build_config.tool_path);
        info!("    SMP: {}", self.build_config.smp);
        if let Some(ref features) = self.features {
            info!("    Features: {}", features.join(", "));
        } else {
            info!("    Features: (default)");
        }
        info!("    Output Dir: {}", self.elf_name().display());

        // Prepare environment variables
        let linker_path = "link.lds";
        let rustflags = format!("-C link-arg=-T{} -C force-frame-pointers=yes", linker_path);
        let log_level = &self.build_config.log;
        let build_time = chrono::Local::now().to_string();
        let smp = self.build_config.smp.to_string();

        // Set environment variables for cargo
        sh.set_var("RUSTFLAGS", &rustflags);
        sh.set_var("TINYENV_LOG", log_level);
        sh.set_var("TINYENV_BUILD_TIME", &build_time);
        sh.set_var("TINYENV_SMP", &smp);

        // Build cargo command
        let target = &self.build_config.target;
        
        info!("==> Execute build command");
        if self.build_config.mode == "release" {
            if let Some(ref features) = self.features {
                let features_str = features.join(",");
                cmd!(sh, "cargo build --release --target {target} --features {features_str}").run()?;
            } else {
                cmd!(sh, "cargo build --release --target {target}").run()?;
            }
        } else {
            if let Some(ref features) = self.features {
                let features_str = features.join(",");
                cmd!(sh, "cargo build --target {target} --features {features_str}").run()?;
            } else {
                cmd!(sh, "cargo build --target {target}").run()?;
            }
        }

        info!("==> Build succeeded!");
        info!("    ELF file: {}", self.elf_name().display());

        // Process debug sections for runtime backtrace support
        info!("==> Processing debug sections");
        let elf_path = self.elf_name();
        crate::dwarf::process_debug_sections(&elf_path)?;
        info!("    Debug sections processed");

        // Generate binary file
        info!("==> Copy binary");
        let elf_path = self.elf_name();
        let bin_path = elf_path.with_extension("bin");
        cmd!(sh, "rust-objcopy -O binary {elf_path} {bin_path}").run()?;
        info!("    Binary file: {}", bin_path.display());

        // Generate disassembly file
        info!("==> Objdump");
        let asm_path = elf_path.with_extension("asm");
        let asm_content = cmd!(sh, "rust-objdump -d --print-imm-hex {elf_path}").read()?;
        std::fs::write(&asm_path, asm_content)?;
        info!("    ASM file: {}", asm_path.display());

        // Generate U-Boot image
        info!("==> Mkimage");
        let uimg_path = elf_path.with_extension("uimg");
        let load_addr = format!("0x{:x}", self.build_config.load_address);
        let entry_point = format!("0x{:x}", self.build_config.entry_point);
        let project_name = &self.project_name;
        
        cmd!(sh, "mkimage -A arm -O linux -T kernel -C none -a {load_addr} -e {entry_point} -n {project_name} -d {bin_path} {uimg_path}").run()?;
        info!("    UIMG file: {}", uimg_path.display());

        Ok(())
    }
}

