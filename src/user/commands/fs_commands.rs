//! Filesystem commands.

use crate::TinyResult;
use crate::user::Command;
use crate::user::CommandContext;
use crate::fs;

/// List directory contents.
pub static LS: LsCommand = LsCommand;

pub struct LsCommand;

impl Command for LsCommand {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn description(&self) -> &'static str {
        "List directory contents"
    }

    fn usage(&self) -> &'static str {
        "Usage: ls [path]"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let path = ctx.args.get(0);
        if let Err(e) = fs::list_dir(path) {
            println!("ls: {}", e);
        }
        Ok(())
    }
}

/// Change directory.
pub static CD: CdCommand = CdCommand;

pub struct CdCommand;

impl Command for CdCommand {
    fn name(&self) -> &'static str {
        "cd"
    }

    fn description(&self) -> &'static str {
        "Change current working directory"
    }

    fn usage(&self) -> &'static str {
        "Usage: cd <path>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        if let Some(path) = ctx.args.get(0) {
            if let Err(e) = fs::change_dir(path) {
                println!("cd: {}", e);
            }
        } else {
            println!("Usage: cd <path>");
        }
        Ok(())
    }
}

/// Make directory.
pub static MKDIR: MkdirCommand = MkdirCommand;

pub struct MkdirCommand;

impl Command for MkdirCommand {
    fn name(&self) -> &'static str {
        "mkdir"
    }

    fn description(&self) -> &'static str {
        "Create a directory"
    }

    fn usage(&self) -> &'static str {
        "Usage: mkdir <path>"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        if let Some(path) = ctx.args.get(0) {
            if let Err(e) = fs::make_dir(path) {
                println!("mkdir: {}", e);
            }
        } else {
            println!("Usage: mkdir <path>");
        }
        Ok(())
    }
}


/// Print current working directory.
pub static PWD: PwdCommand = PwdCommand;

pub struct PwdCommand;

impl Command for PwdCommand {
    fn name(&self) -> &'static str {
        "pwd"
    }

    fn description(&self) -> &'static str {
        "Print current working directory"
    }

    fn usage(&self) -> &'static str {
        "Usage: pwd"
    }

    fn category(&self) -> &'static str {
        "filesystem"
    }

    fn execute(&self, _ctx: &CommandContext) -> TinyResult<()> {
        let cwd = fs::current_dir();
        println!("{}", cwd);
        Ok(())
    }
}
