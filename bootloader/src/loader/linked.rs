//! Linker-owned input ranges and build-time placement configuration.
use super::{BootError, LoadPlan, images::BootImages};
use crate::memory::{self as layout, Region};
use core::ptr::addr_of;

unsafe extern "C" {
    static __archive_start: u8;
    static __archive_end: u8;
    static __loader_start: u8;
    static __loader_end: u8;
}
/// Linker-owned immutable archive, disjoint from the stack and all load targets.
fn linked_archive() -> &'static [u8] {
    // SAFETY: The linker defines an ordered, nonempty allocated section within
    // the loaded bootloader. It stays immutable throughout the loading process.
    unsafe {
        core::slice::from_raw_parts(
            addr_of!(__archive_start),
            addr_of!(__archive_end) as usize - addr_of!(__archive_start) as usize,
        )
    }
}
pub(crate) fn plan() -> Result<LoadPlan<'static>, BootError> {
    let images = BootImages::parse(linked_archive())?;
    let loader = Region::new(
        addr_of!(__loader_start) as usize,
        addr_of!(__loader_end) as usize - addr_of!(__loader_start) as usize,
    )?;
    let text = env!("KERNEL_LOAD_MIN");
    let minimum = match text.strip_prefix("0x") {
        Some(hex) => usize::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|_| layout::Error::LoadMinimum)?;
    LoadPlan::new(images, loader, minimum)
}
