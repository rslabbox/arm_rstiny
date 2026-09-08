//! Static AArch64 ELF loading into a fresh child address space.
use crate::{Error, Permissions, Task, elf_image::Elf};

const PAGE: usize = 4096;
const STACK_END: usize = 0x0800_0000;
const STACK_SIZE: usize = 16 * 1024;
const STACK_START: usize = STACK_END - STACK_SIZE;

/// Load and start a static ELF with a private 16 KiB stack and x0=0.
/// The executable uses the ordinary `#[entry] fn() -> !` runtime contract.
/// Failed loads destroy the child and release every allocated page.
pub fn spawn(image: &[u8]) -> Result<Task, Error> {
    let elf = Elf::parse(image).map_err(|_| Error::InvalidArgument)?;
    let pages: usize = elf.segments().map(|s| (s.end - s.va) / PAGE).sum();
    if elf.start < 0x0040_0000 || elf.end > STACK_START - PAGE || pages > 1020 {
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
            child.map(STACK_START, STACK_SIZE, Permissions::ReadWrite)?;
            child.start(elf.entry, STACK_END, 0)?;
        }
        Ok(child)
    })();
    if result.is_err() {
        child.destroy()?;
    }
    result
}
