use crate::utils::TaskResult;
use serde::Deserialize;
use xshell::Shell;

#[derive(Debug, Deserialize, Default)]
struct RunConfig {
    memory: String,
    cpu: String,
    machine: String,
    disk_image: String,
    tcp_port: u16,
    udp_port: u16,
    nographic: bool,
}

pub struct RunTask {
    build: super::build::BuildTask,
    run_config: RunConfig,
    debug: bool,
}

impl RunTask {
    pub fn new(options: crate::BuildOptions, config: &toml::Value) -> TaskResult<Self> {
        let run_config: RunConfig = config
            .get("run")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_default();

        let debug = options.debug;
        let build = super::build::BuildTask::new(options, config)?;

        Ok(RunTask {
            build,
            run_config,
            debug,
        })
    }

    pub fn execute(&self) -> TaskResult<()> {
        info!("==> Building project before running QEMU:");

        // First, execute the build task
        self.build.execute()?;

        info!("==> Starting QEMU:");

        // Get the binary file path
        let bin_path = self.build.elf_name().with_extension("bin");
        info!("    Kernel: {}", bin_path.display());

        let sh = Shell::new()?;
        let project_root = crate::utils::project_root();
        sh.change_dir(&project_root);

        // Build QEMU command
        let memory = &self.run_config.memory;
        let cpu = &self.run_config.cpu;
        let machine = &self.run_config.machine;
        let smp = self.build.smp();
        let disk_image = &self.run_config.disk_image;
        let tcp_port = self.run_config.tcp_port;
        let udp_port = self.run_config.udp_port;

        info!("    Memory: {}", memory);
        info!("    CPU: {}", cpu);
        info!("    Machine: {}", machine);
        info!("    SMP: {}", smp);
        info!("    Disk Image: {}", disk_image);
        info!(
            "    Port Forward: tcp:{}->5555, udp:{}->5555",
            tcp_port, udp_port
        );

        // Construct port forwarding strings
        let tcp_forward = format!("tcp::{}-:5555", tcp_port);
        let udp_forward = format!("udp::{}-:5555", udp_port);
        let netdev = format!(
            "user,id=net0,hostfwd={},hostfwd={}",
            tcp_forward, udp_forward
        );

        // Execute QEMU command
        let mut args = vec![
            "-m".to_string(),
            memory.clone(),
            "-cpu".to_string(),
            cpu.clone(),
            "-machine".to_string(),
            machine.clone(),
            "-kernel".to_string(),
            bin_path.to_string_lossy().to_string(),
            "-device".to_string(),
            "virtio-blk-device,drive=disk0".to_string(),
            "-drive".to_string(),
            format!("id=disk0,if=none,format=raw,file={}", disk_image),
            "-device".to_string(),
            "virtio-net-device,netdev=net0".to_string(),
            "-netdev".to_string(),
            netdev,
        ];

        if smp > 1 {
            args.push("-smp".to_string());
            args.push(smp.to_string());
        }

        if self.debug {
            info!("    Debug Mode: Enabled (-s -S)");
            args.push("-s".to_string());
            args.push("-S".to_string());
        }

        if self.run_config.nographic {
            args.push("-nographic".to_string());
        }

        let cmds = String::from("qemu-system-aarch64 ") + &args.join(" ");
        info!("    QEMU Command: {}", cmds);

        duct::cmd!("bash", "-c", cmds).run()?;

        Ok(())
    }
}
