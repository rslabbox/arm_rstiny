//! Echo command - prints arguments back to the console.

use alloc::string::String;

use crate::TinyResult;
use crate::user::Command;
use crate::user::CommandContext;
use crate::user::commands::env::get_var;

/// Echo command instance.
pub static ECHO: EchoCommand = EchoCommand;

/// Echo command implementation.
pub struct EchoCommand;

impl Command for EchoCommand {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echo text back to the console (supports $var substitution)"
    }

    fn usage(&self) -> &'static str {
        "Usage: echo <text>\r\n\
         \r\n\
         Prints the given text to the console.\r\n\
         Supports variable substitution with $name syntax.\r\n\
         \r\n\
         Example:\r\n\
           env set greeting Hello\r\n\
           echo $greeting world  -> prints 'Hello world'"
    }

    fn category(&self) -> &'static str {
        "general"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        let output = expand_variables(ctx.args_raw);
        println!("{}", output);
        Ok(())
    }
}

/// Expand $var references in a string.
fn expand_variables(input: &str) -> String {
    let mut result = String::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            // Collect variable name (alphanumeric and underscore)
            let mut var_name = String::new();
            while let Some(&nc) = chars.peek() {
                if nc.is_alphanumeric() || nc == '_' {
                    var_name.push(nc);
                    chars.next();
                } else {
                    break;
                }
            }

            if var_name.is_empty() {
                // Lone '$' with no variable name
                result.push('$');
            } else {
                // Substitute variable value or empty string
                if let Some(value) = get_var(&var_name) {
                    result.push_str(&value);
                }
                // If variable not found, replace with empty string
            }
        } else {
            result.push(c);
        }
    }

    result
}
