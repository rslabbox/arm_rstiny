//! Errors reported before any image is committed to physical RAM.
use crate::{image::device_tree, memory as layout};
use core::fmt;

use rstiny_newc::ArchiveError;

#[derive(Debug)]
pub(crate) enum BootError {
    Archive(ArchiveError),
    KernelElf(rstiny_elf::Error),
    RootElf(rstiny_elf::Error),
    DeviceTree(device_tree::Error),
    Layout(layout::Error),
}
impl From<layout::Error> for BootError {
    fn from(e: layout::Error) -> Self {
        Self::Layout(e)
    }
}
impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(e) => e.fmt(f),
            Self::KernelElf(e) => write!(f, "kernel ELF: {e}"),
            Self::RootElf(e) => write!(f, "root ELF: {e}"),
            Self::DeviceTree(e) => write!(f, "DTB: {e:?}"),
            Self::Layout(e) => write!(f, "layout: {e:?}"),
        }
    }
}
