//! Command registry - static command registration and lookup.

use crate::user::command::{Command, CommandContext};
use crate::user::commands;

/// Static array of all registered commands.
///
/// To add a new command:
/// 1. Create a new file in `commands/` directory
/// 2. Implement the `Command` trait
/// 3. Export a static instance
/// 4. Add it to this array
static COMMANDS: &[&dyn Command] = &[
    &commands::HELP,
    &commands::ECHO,
    &commands::ENV,
    &commands::HISTORY_CMD,
    &commands::TEST,
    &commands::EXIT,
    &commands::LS,
    &commands::CD,
    &commands::MKDIR,
    &commands::PWD,
];

/// Find a command by name or alias.
pub fn find_command(name: &str) -> Option<&'static dyn Command> {
    for cmd in COMMANDS {
        if cmd.name() == name {
            return Some(*cmd);
        }
        for alias in cmd.aliases() {
            if *alias == name {
                return Some(*cmd);
            }
        }
    }
    None
}

/// Get all registered commands.
pub fn all_commands() -> &'static [&'static dyn Command] {
    COMMANDS
}

/// Execute a command line.
///
/// Parses the input, finds the matching command, and executes it.
pub fn execute(line: &str) {
    let Some(ctx) = CommandContext::parse(line) else {
        return;
    };

    if let Some(cmd) = find_command(ctx.command) {
        let _ = cmd.execute(&ctx);
    } else {
        println!("Unknown command: {}", ctx.command);
        println!("Type 'help' to see available commands.");
    }
}
