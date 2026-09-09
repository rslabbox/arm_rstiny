//! Architecture-specific startup and kernel transfer.
mod aarch64;
pub(crate) use aarch64::{bootloader_halt, enter};
