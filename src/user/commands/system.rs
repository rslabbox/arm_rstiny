//! System commands - power management and system control.

use crate::TinyResult;
use crate::user::{Command, CommandContext};

/// Exit/poweroff command instance.
pub static EXIT: ExitCommand = ExitCommand;

/// Exit/poweroff command implementation.
pub struct ExitCommand;

impl Command for ExitCommand {
    fn name(&self) -> &'static str {
        "exit"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["poweroff", "off", "reboot"]
    }

    fn description(&self) -> &'static str {
        "Power off the system (PSCI system_off)"
    }

    fn usage(&self) -> &'static str {
        "Usage: exit\r\n\
         Aliases: poweroff, off, reboot\r\n\
         \r\n\
         Powers off the system using PSCI system_off call."
    }

    fn category(&self) -> &'static str {
        "system"
    }

    fn execute(&self, _ctx: &CommandContext) -> TinyResult<()> {
        println!("Powering off...");
        crate::drivers::power::system_off();
    }
}
