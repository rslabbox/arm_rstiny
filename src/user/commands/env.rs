//! Environment variable command - manage shell variables.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};

use crate::hal::Mutex;
use crate::user::{Command, CommandContext};
use crate::TinyResult;

/// Global environment variables storage.
pub static ENV_VARS: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Get an environment variable value.
pub fn get_var(name: &str) -> Option<String> {
    ENV_VARS.lock().get(name).cloned()
}

/// Set an environment variable.
pub fn set_var(name: &str, value: &str) {
    ENV_VARS.lock().insert(name.to_string(), value.to_string());
}

/// Remove an environment variable.
pub fn unset_var(name: &str) -> Option<String> {
    ENV_VARS.lock().remove(name)
}

/// Env command instance.
pub static ENV: EnvCommand = EnvCommand;

/// Env command implementation.
pub struct EnvCommand;

impl Command for EnvCommand {
    fn name(&self) -> &'static str {
        "env"
    }

    fn description(&self) -> &'static str {
        "Manage environment variables"
    }

    fn usage(&self) -> &'static str {
        "Usage: env [subcommand] [args...]\r\n\
         \r\n\
         Subcommands:\r\n\
           env              - List all variables\r\n\
           env get <name>   - Get variable value\r\n\
           env set <n> <v>  - Set variable\r\n\
           env unset <name> - Remove variable\r\n\
         \r\n\
         Example:\r\n\
           env set name test\r\n\
           echo $name        -> prints 'test'"
    }

    fn category(&self) -> &'static str {
        "general"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        match ctx.args.get(0) {
            None => {
                // List all variables
                let vars = ENV_VARS.lock();
                if vars.is_empty() {
                    println!("No environment variables set.");
                } else {
                    println!("Environment variables:");
                    for (name, value) in vars.iter() {
                        println!("  {}={}", name, value);
                    }
                }
                Ok(())
            }
            Some("get") => {
                use anyhow::Context;
                let name = ctx
                    .args
                    .get(1)
                    .context("Usage: env get <name>")?;
                match get_var(name) {
                    Some(value) => {
                        println!("{}", value);
                        Ok(())
                    }
                    None => {
                        println!("Variable '{}' not set", name);
                        anyhow::bail!("Variable not found: {}", name)
                    }
                }
            }
            Some("set") => {
                use anyhow::Context;
                let name = ctx.args.get(1).context(
                    "Usage: env set <name> <value>",
                )?;
                // Join remaining args as value (allows spaces in value)
                let value = if ctx.args.len() > 2 {
                    // Get everything after "set <name> "
                    let prefix = alloc::format!("set {} ", name);
                    ctx.args_raw.strip_prefix(&prefix).unwrap_or("")
                } else {
                    ""
                };
                set_var(name, value);
                println!("{}={}", name, value);
                Ok(())
            }
            Some("unset") => {
                use anyhow::Context;
                let name = ctx
                    .args
                    .get(1)
                    .context("Usage: env unset <name>")?;
                if unset_var(name).is_some() {
                    println!("Unset '{}'", name);
                    Ok(())
                } else {
                    println!("Variable '{}' not set", name);
                    anyhow::bail!("Variable not found: {}", name)
                }
            }
            Some(unknown) => {
                println!("Unknown subcommand: {}", unknown);
                println!("Type 'help env' for usage.");
                anyhow::bail!("Unknown subcommand: {}", unknown)
            }
        }
    }
}
