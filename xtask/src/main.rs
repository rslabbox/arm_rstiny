use std::process::exit;
use std::{env, fs};

mod utils;
use utils::{project_root, TaskResult};

mod plugins;

#[macro_use]
extern crate log;

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
    let mut args = env::args().skip(1).collect::<Vec<_>>();

    if args.is_empty() {
        print_help();
        return Ok(());
    }

    let task_name = args.remove(0);

    // 先加载配置
    let config = load_config()?;

    // Fetch and execute the task plugin
    let plugin = plugins::fetch_task(&task_name, &args, &config)?;
    plugin.execute()?;

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

fn print_help() {
    println!("xtask - project auxiliary tasks");
    println!("");
    println!("Usage:");
    println!("  cargo xtask <task> [<options>]");
    println!("");
    println!("Tasks:");
    for task in plugins::list_tasks() {
        println!("  {}", task);
        println!("    {}", plugins::task_descriptions(task));
    }
    println!("");
    println!("Use `cargo xtask <task> --help` for more information on a specific task.");
}
