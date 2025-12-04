//! Command trait and context definitions.

use alloc::vec::Vec;

use crate::TinyResult;

/// Parsed command arguments.
pub struct Args<'a> {
    args: Vec<&'a str>,
}

impl<'a> Args<'a> {
    /// Create Args from a slice of string references.
    pub fn new(args: Vec<&'a str>) -> Self {
        Self { args }
    }

    /// Get argument at index (0 is first argument after command name).
    pub fn get(&self, index: usize) -> Option<&'a str> {
        self.args.get(index).copied()
    }

    /// Number of arguments.
    pub fn len(&self) -> usize {
        self.args.len()
    }

    /// Check if no arguments.
    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    /// Iterate over arguments.
    pub fn iter(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.args.iter().copied()
    }

    /// Get all arguments as a single string (joined by space).
    pub fn rest(&self) -> Option<&'a str> {
        // This returns the first arg which contains the rest of the line
        // For proper implementation, we store the rest separately
        None
    }

    /// Get the raw rest of the line after command name.
    pub fn raw_rest(&self) -> &'a str {
        // Will be set by the parser
        ""
    }
}

/// Command execution context.
pub struct CommandContext<'a> {
    /// The original raw input line.
    pub raw: &'a str,
    /// The command name that was invoked.
    pub command: &'a str,
    /// Parsed arguments (excluding command name).
    pub args: Args<'a>,
    /// Raw argument string (everything after command name).
    pub args_raw: &'a str,
}

impl<'a> CommandContext<'a> {
    /// Create a new command context by parsing a line.
    pub fn parse(line: &'a str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        // Split into command and rest
        let mut parts = line.splitn(2, ' ');
        let command = parts.next()?;
        let args_raw = parts.next().unwrap_or("").trim_start();

        // Parse arguments (simple space-split for now)
        let args: Vec<&str> = if args_raw.is_empty() {
            Vec::new()
        } else {
            args_raw.split_whitespace().collect()
        };

        Some(Self {
            raw: line,
            command,
            args: Args::new(args),
            args_raw,
        })
    }
}

/// Trait for implementing commands.
///
/// Commands are registered statically and looked up by name or alias.
pub trait Command: Sync {
    /// Primary command name.
    fn name(&self) -> &'static str;

    /// Alternative names for this command.
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    /// Short description (shown in help list).
    fn description(&self) -> &'static str;

    /// Detailed usage information (shown in `help <command>`).
    fn usage(&self) -> &'static str {
        self.description()
    }

    /// Command category for grouping in help.
    fn category(&self) -> &'static str {
        "general"
    }

    /// Execute the command with the given context.
    fn execute(&self, ctx: &CommandContext) -> TinyResult<()>;
}
