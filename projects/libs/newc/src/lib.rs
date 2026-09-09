#![no_std]
//! Shared newc CPIO decoding for the bootloader and userland boot modules.
//!
//! The cursor performs bounded, allocation-free reads; [`BootArchive`] layers
//! the boot-image contract (three fixed images in order, then named modules,
//! then the terminal record). Contents are borrowed, never copied.
mod archive;
pub use archive::{ArchiveError, ArchiveErrorKind, BootArchive, MAX_MODULES};
mod newc;
pub use newc::{Entry, Record};
