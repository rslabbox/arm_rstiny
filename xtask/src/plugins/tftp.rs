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

impl TftpTask {
    pub fn new(options: crate::BuildOptions, config: &toml::Value) -> TaskResult<Self> {
        let tftp_config: TftpConfig = config
            .get("tftp")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default();

        let build = super::build::BuildTask::new(options, config)?;

        Ok(TftpTask { build, tftp_config })
    }

    pub fn execute(&self) -> TaskResult<()> {
        info!("==> Uploading UIMG file via TFTP:");
        let uimg_path = self.build.elf_name().with_extension("uimg");
        info!("    UIMG Path: {}", uimg_path.display());

        self.build.execute()?;

        // Copy file to TFTP directory
        let tftp_path = PathBuf::from(&self.tftp_config.tftp_path);

        info!("    TFTP Path: {}", tftp_path.display());

        fs::copy(&uimg_path, &tftp_path)?;
        info!("    Copied to: {}", tftp_path.display());
        info!("    Upload successful!");

        Ok(())
    }
}
