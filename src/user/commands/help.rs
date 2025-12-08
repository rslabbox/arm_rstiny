//! Help command - displays available commands and their usage.

use crate::user::{Command, CommandContext};
use crate::TinyResult;

/// Help command instance.
pub static HELP: HelpCommand = HelpCommand;

/// Help command implementation.
pub struct HelpCommand;

impl Command for HelpCommand {
    fn name(&self) -> &'static str {
        "help"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["?"]
    }

    fn description(&self) -> &'static str {
        "Show available commands or help for a specific command"
    }

    fn usage(&self) -> &'static str {
        "Usage: help [command]\r\n\
         \r\n\
         Without arguments: lists all available commands.\r\n\
         With a command name: shows detailed help for that command."
    }

    fn category(&self) -> &'static str {
        "general"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        if let Some(cmd_name) = ctx.args.get(0) {
            // Show help for specific command
            show_command_help(cmd_name)
        } else {
            // List all commands
            show_all_commands()
        }
    }
}

fn show_command_help(name: &str) -> TinyResult<()> {
    if let Some(cmd) = crate::user::registry::find_command(name) {
        print!("Command: {}", cmd.name());

        let aliases = cmd.aliases();
        if !aliases.is_empty() {
            print!(" (aliases: ");
            for (i, alias) in aliases.iter().enumerate() {
                if i > 0 {
                    print!(", ");
                }
                print!("{}", alias);
            }
            print!(")");
        }
        println!();

        println!("{}", cmd.usage());

        Ok(())
    } else {
        println!("Unknown command: {}", name);
        println!("Type 'help' to see available commands.");
        anyhow::bail!("Command not found: {}", name)
    }
}

fn show_all_commands() -> TinyResult<()> {
    println!("Available commands:\r\n");

    // Group commands by category
    let commands = crate::user::registry::all_commands();

    // Collect unique categories
    let mut categories: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();
    for cmd in commands {
        let cat = cmd.category();
        if !categories.contains(&cat) {
            categories.push(cat);
        }
    }

    // Sort categories (general first, then alphabetically)
    categories.sort_by(|a, b| {
        if *a == "general" {
            core::cmp::Ordering::Less
        } else if *b == "general" {
            core::cmp::Ordering::Greater
        } else {
            a.cmp(b)
        }
    });

    // Print commands by category
    for category in categories {
        println!("[{}]", category);

        for cmd in commands {
            if cmd.category() == category {
                println!("  {:12} - {}", cmd.name(), cmd.description());
            }
        }
        println!();
    }

    println!("Type 'help <command>' for detailed usage.");
    Ok(())
}
