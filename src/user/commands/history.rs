//! Command history management and history command.

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};

use crate::TinyResult;
use crate::hal::Mutex;
use crate::user::{Command, CommandContext};

/// Maximum number of history entries to keep.
const MAX_HISTORY: usize = 32;

/// Global command history storage.
pub static HISTORY: Mutex<CommandHistory> = Mutex::new(CommandHistory::new());

/// Command history buffer.
pub struct CommandHistory {
    /// Stored commands (newest at back).
    entries: VecDeque<String>,
    /// Current navigation index (0 = newest, increases going back).
    index: usize,
    /// Temporary storage for current line when navigating.
    current_line: String,
    /// Whether we're currently navigating history.
    navigating: bool,
}

impl CommandHistory {
    /// Create a new empty history.
    pub const fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            index: 0,
            current_line: String::new(),
            navigating: false,
        }
    }

    /// Add a command to history.
    pub fn push(&mut self, cmd: &str) {
        // Don't add empty commands or duplicates of the last command
        if cmd.is_empty() {
            return;
        }
        if let Some(last) = self.entries.back() {
            if last == cmd {
                return;
            }
        }

        self.entries.push_back(cmd.to_string());

        // Limit history size
        while self.entries.len() > MAX_HISTORY {
            self.entries.pop_front();
        }

        // Reset navigation state
        self.reset_navigation();
    }

    /// Start navigating history, saving the current line.
    pub fn start_navigation(&mut self, current: &str) {
        if !self.navigating {
            self.current_line = current.to_string();
            self.navigating = true;
            self.index = 0;
        }
    }

    /// Reset navigation state.
    pub fn reset_navigation(&mut self) {
        self.navigating = false;
        self.index = 0;
        self.current_line.clear();
    }

    /// Navigate to previous (older) command. Returns the command if available.
    pub fn prev(&mut self, current: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }

        self.start_navigation(current);

        if self.index < self.entries.len() {
            self.index += 1;
            let entry_idx = self.entries.len() - self.index;
            Some(&self.entries[entry_idx])
        } else {
            // Already at oldest entry
            Some(&self.entries[0])
        }
    }

    /// Navigate to next (newer) command. Returns the command or current line.
    pub fn next(&mut self) -> Option<&str> {
        if !self.navigating {
            return None;
        }

        if self.index > 1 {
            self.index -= 1;
            let entry_idx = self.entries.len() - self.index;
            Some(&self.entries[entry_idx])
        } else if self.index == 1 {
            // Return to current line
            self.index = 0;
            Some(&self.current_line)
        } else {
            // Already at current line
            Some(&self.current_line)
        }
    }

    /// Get all history entries (oldest first).
    pub fn entries(&self) -> impl Iterator<Item = &String> {
        self.entries.iter()
    }

    /// Get the number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// History command instance.
pub static HISTORY_CMD: HistoryCommand = HistoryCommand;

/// History command implementation.
pub struct HistoryCommand;

impl Command for HistoryCommand {
    fn name(&self) -> &'static str {
        "history"
    }

    fn description(&self) -> &'static str {
        "Show command history"
    }

    fn usage(&self) -> &'static str {
        "Usage: history [clear]\r\n\
         \r\n\
         Without arguments: shows all command history.\r\n\
         With 'clear': clears the history.\r\n\
         \r\n\
         Use Up/Down arrow keys to navigate history."
    }

    fn category(&self) -> &'static str {
        "general"
    }

    fn execute(&self, ctx: &CommandContext) -> TinyResult<()> {
        match ctx.args.get(0) {
            Some("clear") => {
                let mut history = HISTORY.lock();
                history.entries.clear();
                history.reset_navigation();
                println!("History cleared.");
            }
            Some(_) => {
                println!("Unknown argument. Usage: history [clear]");
            }
            None => {
                let history = HISTORY.lock();
                if history.is_empty() {
                    println!("No command history.");
                } else {
                    println!("Command history:");
                    for (i, cmd) in history.entries().enumerate() {
                        println!("  {:3}  {}", i + 1, cmd);
                    }
                }
            }
        }
        Ok(())
    }
}
