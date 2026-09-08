//! Static AArch64 ELF loading into a fresh child address space.
use crate::{Error, Permissions, Task};
use rstiny_elf::Elf;

const PAGE: usize = 4096;
const STACK_SIZE: usize = 16 * 1024;

/// Load and start a static ELF with a private 16 KiB stack and x0=0.
/// The executable uses the ordinary `#[entry] fn() -> !` runtime contract.
/// Failed loads destroy the child and release every allocated page.
pub fn spawn(image: &[u8]) -> Result<Task, Error> {
    let elf = Elf::parse(image).map_err(|_| Error::InvalidArgument)?;
    if elf.segments().any(|s| !matches!(s.flags, 4..=6)) {
        return Err(Error::InvalidArgument);
    }
    let pages: usize = elf.segments().map(|s| (s.end - s.va) / PAGE).sum();
    let stack_bottom = elf.end().checked_add(PAGE).ok_or(Error::InvalidArgument)?;
    let stack_top = stack_bottom
        .checked_add(STACK_SIZE)
        .ok_or(Error::InvalidArgument)?;
    if elf.start() < PAGE
        || stack_top > kernel_abi::USER_ADDRESS_LIMIT as usize
        || pages + STACK_SIZE / PAGE > kernel_abi::MAX_USER_PAGES
    {
        return Err(Error::InvalidArgument);
    }
    let child = Task::create()?;
    let result = (|| {
        for segment in elf.segments() {
            let size = segment.end - segment.va;
            // The child is stopped and exclusively owned during construction.
            // Pages start zeroed (including BSS), writable but not executable.
            unsafe {
                child.map(segment.va, size, Permissions::ReadWrite)?;
            }
            for (index, data) in image[segment.offset..segment.offset + segment.filesz]
                .chunks(PAGE)
                .enumerate()
            {
                unsafe {
                    child.write_memory(segment.va + index * PAGE, data)?;
                }
            }
            let rights = match segment.flags {
                4 => Permissions::Read,
                5 => Permissions::ReadExecute,
                6 => Permissions::ReadWrite,
                _ => unreachable!(),
            };
            unsafe {
                child.protect(segment.va, size, rights)?;
            }
        }
        unsafe {
            child.map(stack_bottom, STACK_SIZE, Permissions::ReadWrite)?;
            child.start(elf.entry(), stack_top, 0)?;
        }
        Ok(child)
    })();
    if result.is_err() {
        child.destroy()?;
    }
    result
}
