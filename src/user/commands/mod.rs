//! Command implementations.
//!
//! Each command is defined in its own module file.

pub mod echo;
pub mod env;
pub mod fs_commands;
pub mod help;
pub mod history;
pub mod system;
pub mod test;

// Re-export command instances for registry
pub use echo::ECHO;
pub use env::ENV;
pub use fs_commands::{
    CAT, CD, CHMOD, CP, LN, LS, MKDIR, MV, PWD, RM, RMDIR, STAT, TOUCH, TREE, WRITE,
};
pub use help::HELP;
pub use history::HISTORY_CMD;
pub use system::EXIT;
pub use test::TEST;
