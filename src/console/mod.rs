//! Console module - Print and logging facilities.
//!
//! This module provides console output and logging support.

pub mod logger;

#[macro_use]
pub mod print;

pub use logger::init as init_logger;
