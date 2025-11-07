use clap::{Parser, Subcommand};
use std::fs;
use std::process::exit;

mod utils;
use utils::{project_root, TaskResult};

mod plugins;

#[macro_use]
extern crate log;

#[derive(Debug, Parser)]
#[command(name = "xtask")]
#[command(about = "Project auxiliary tasks", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Parser)]
pub struct BuildOptions {
    /// Log level (trace, debug, info, warn, error)
    #[arg(long)]
    pub log: Option<String>,

    /// Build mode (debug/release)
    #[arg(long)]
    pub mode: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Build the project with specified configurations
    Build(BuildOptions),

    /// Upload the built image file to TFTP server
    Tftp(BuildOptions),

    /// Flash the built image to the target device
    Flash(BuildOptions),
}

fn main() {
    // 初始化 env_logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .init();

    if let Err(e) = try_main() {
        match e {
            crate::utils::TaskError::Ok => {
                // Do nothing on Ok
            }
            _ => {
                log::error!("{}", e);
                exit(1);
            }
        }
    }
}

fn try_main() -> TaskResult<()> {
    let cli = Cli::parse();

    // 先加载配置
    let config = load_config()?;

    match cli.command {
        Commands::Build(options) => {
            let task = plugins::build::BuildTask::new(options, &config)?;
            task.execute()?;
        }
        Commands::Tftp(options) => {
            let task = plugins::tftp::TftpTask::new(options, &config)?;
            task.execute()?;
        }
        Commands::Flash(options) => {
            let task = plugins::flash::FlashTask::new(options, &config)?;
            task.execute()?;
        }
    }

    Ok(())
}

fn load_config() -> TaskResult<toml::Value> {
    let project_root = project_root();
    let config_path = project_root.join("tinyconfig.toml");

    if !config_path.exists() {
        return Err(crate::utils::TaskError::ConfigNotFound);
    }

    let content = fs::read_to_string(&config_path)?;
    let config: toml::Value = toml::from_str(&content)?;

    Ok(config)
}
