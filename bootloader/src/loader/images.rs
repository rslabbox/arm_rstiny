//! Decode the three boot payloads before planning destinations.
use super::BootError;
use crate::image::{device_tree::DeviceTree, elf::Elf};
use rstiny_newc::BootArchive;

pub(super) struct BootImages<'a> {
    pub(super) kernel: Elf<'a>,
    pub(super) root: Elf<'a>,
    pub(super) dtb: DeviceTree<'a>,
    /// The complete archive bytes, copied verbatim for the root task.
    pub(super) raw: &'a [u8],
}
impl<'a> BootImages<'a> {
    pub(super) fn parse(bytes: &'a [u8]) -> Result<Self, BootError> {
        let archive = BootArchive::parse(bytes).map_err(BootError::Archive)?;
        Ok(Self {
            kernel: Elf::parse(archive.kernel()).map_err(BootError::KernelElf)?,
            root: Elf::parse(archive.rootserver()).map_err(BootError::RootElf)?,
            dtb: DeviceTree::parse(archive.device_tree()).map_err(BootError::DeviceTree)?,
            raw: bytes,
        })
    }
}
