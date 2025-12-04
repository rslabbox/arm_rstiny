//! User command system - Modular command architecture.
//!
//! This module provides a framework for defining and executing shell commands.
//!
//! # Architecture
//!
//! - `command.rs` - Defines the `Command` trait and execution context
//! - `registry.rs` - Static command registration and lookup
//! - `commands/` - Individual command implementations
//!
//! # Adding a New Command
//!
//! 1. Create a new file in `commands/` (e.g., `commands/mycommand.rs`)
//! 2. Define a struct and implement the `Command` trait
//! 3. Export a static instance: `pub static MYCOMMAND: MyCommand = MyCommand;`
//! 4. Add to `commands/mod.rs`: `pub mod mycommand;` and `pub use mycommand::MYCOMMAND;`
//! 5. Register in `registry.rs` COMMANDS array: `&commands::MYCOMMAND,`
#![allow(unused)]

pub mod command;
pub mod commands;
pub mod registry;

pub use command::{Args, Command, CommandContext};
pub use registry::execute;
