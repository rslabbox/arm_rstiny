//! RstinyOS - Main kernel entry point.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod boot;
mod config;

mod console;
mod error;

mod drivers;
mod hal;
mod mm;
mod platform;
mod task;
mod tests;

// Future modules (placeholder)
mod fs;

#[cfg(feature = "net")]
mod net;
mod sync;

#[macro_use]
extern crate log;

extern crate alloc;
extern crate axbacktrace;

pub use error::{TinyError, TinyResult};

/// User main task entry point.
fn user_main() {
    // Run tests in main task
    tests::rstiny_tests();


    info!("User main task completed");
}
