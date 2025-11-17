#![allow(unused)]
use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum TaskError {
    #[error("Command not found: {0}")]
    CmdNotFound(String),
    #[error("Config not found")]
    ConfigNotFound,
    #[error("Task not found: {0}")]
    TaskNotFound(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Unknown argument: {0}")]
    UnknownArgument(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("figment: {0}")]
    Figment(#[from] figment::Error),
    #[error("xshell: {0}")]
    Shell(#[from] xshell::Error),
    #[error("network: {0}")]
    Network(String),
    #[error("Ok")]
    Ok,
}

pub type TaskResult<T> = Result<T, TaskError>;

/// Project root directory
pub fn project_root() -> PathBuf {
    std::path::Path::new(&env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(1)
        .unwrap()
        .to_path_buf()
}

/// Xtask directory
pub fn xtask_root() -> PathBuf {
    std::path::Path::new(&env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
