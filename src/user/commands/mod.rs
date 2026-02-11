//! Command implementations.
//!
//! Each command is defined in its own module file.

pub mod echo;
pub mod env;
pub mod help;
pub mod history;
pub mod system;
pub mod test;
pub mod fs_commands;

// Re-export command instances for registry
pub use echo::ECHO;
pub use env::ENV;
pub use help::HELP;
pub use history::HISTORY_CMD;
pub use system::EXIT;
pub use test::TEST;
pub use fs_commands::{LS, CD, MKDIR, PWD, CAT, TOUCH, RM, RMDIR, CP, MV, LN, STAT, CHMOD, TREE, WRITE};
