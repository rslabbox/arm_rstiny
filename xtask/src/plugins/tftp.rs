use crate::utils::TaskResult;
use serde::Deserialize;
use std::{fs, path::PathBuf};

#[derive(Debug, Deserialize, Default)]
struct TftpConfig {
    tftp_path: String,
}

pub struct TftpTask {
    build: super::build::BuildTask,
    tftp_config: TftpConfig,
}

impl super::TaskPlugin for TftpTask {
    fn new(args: &[String], config: &toml::Value) -> Self {
        let tftp_config: TftpConfig = config
            .get("tftp")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default();

        TftpTask {
            build: super::build::BuildTask::new(args, config),
            tftp_config,
        }
    }

    fn description() -> &'static str {
        "Upload the built ELF file to the target device via TFTP"
    }

    fn execute(&self) -> TaskResult<()> {
        info!("==> Uploading UIMG file via TFTP:");
        let uimg_path = self.build.elf_name().with_extension("uimg");
        info!("    UIMG Path: {}", uimg_path.display());
        if !uimg_path.exists() {
            self.build.execute()?;
        }
        // Copy file to /data/docker/tftpboot/
        let tftp_path = PathBuf::from(&self.tftp_config.tftp_path);
        fs::copy(&uimg_path, &tftp_path)?;
        info!("    Copied to: {}", tftp_path.display());
        info!("    Upload successful!");

        Ok(())
    }
}

pub use TftpTask as TaskInstance;
