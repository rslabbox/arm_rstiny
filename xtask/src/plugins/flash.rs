use crate::utils::{TaskResult, project_root};
use xshell::{Shell, cmd};

pub struct FlashTask {
    build: super::build::BuildTask,
}

impl FlashTask {
    pub fn new(options: crate::BuildOptions, config: &toml::Value) -> TaskResult<Self> {
        let build = super::build::BuildTask::new(options, config)?;

        Ok(FlashTask { build })
    }

    pub fn execute(&self) -> TaskResult<()> {
        info!("==> Flashing UIMG file via Flash:");

        let sh = Shell::new()?;
        let project_root = project_root();
        sh.change_dir(&project_root);

        self.build.execute()?;

        let uimg_path = self
            .build
            .elf_name()
            .with_extension("uimg")
            .display()
            .to_string();

        cmd!(sh, "bash tools/orangepi5/make_flash.sh uimg={uimg_path}").run()?;

        Ok(())
    }
}
