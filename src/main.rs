//! RstinyOS - Main kernel entry point.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(const_result_trait_fn)]
#![feature(const_option_ops)]
#![feature(const_trait_impl)]

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

#[macro_use]
extern crate log;

extern crate alloc;
extern crate axbacktrace;

pub use error::{TinyError, TinyResult};

/// User main task entry point.
fn main() {
    // Run tests in main task
    tests::rstiny_tests();

    debug!("User main task completed");
}
