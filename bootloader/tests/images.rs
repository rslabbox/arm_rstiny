//! Host tests share the production module tree without the AArch64 entry.
#![allow(dead_code)]
#[path = "../src/image/mod.rs"]
mod image;
#[path = "../src/loader/mod.rs"]
mod loader;
#[path = "../src/memory/mod.rs"]
mod memory;
#[path = "../src/platform.rs"]
mod platform;
