//! Host tests for the pure parser/planner modules, without the AArch64 entry.
#![allow(dead_code)]
#[path = "../src/archive.rs"]
mod archive;
#[path = "../src/boot_info.rs"]
mod boot_info;
#[path = "../src/device_tree.rs"]
mod device_tree;
#[path = "../src/elf.rs"]
mod elf;
#[path = "../src/image.rs"]
mod image;
#[path = "../src/layout.rs"]
mod layout;
#[path = "../src/platform.rs"]
mod platform;
