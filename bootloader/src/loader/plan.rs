//! Pure placement validation: constructing a plan never writes destination RAM.
use super::{BootError, images::BootImages};
use crate::{
    image::elf::{PAGE, page_up},
    memory::{self as layout, ImageMapping, Region},
    platform,
};
use kernel_abi::{InitialTaskLayout, MAX_USER_PAGES};

/// Only planning can construct this object. It owns every validated destination
/// and consumes itself when committing, preserving validate-before-write ordering.
pub(crate) struct LoadPlan<'a> {
    pub(super) images: BootImages<'a>,
    pub(super) kernel: ImageMapping,
    pub(super) root: ImageMapping,
    pub(super) dtb: Region,
    pub(super) headers: Region,
    /// Verbatim copy of the whole boot archive for the root task. Optional:
    /// a zero-size region means the build shipped no extra modules and the
    /// handoff reports the archive as absent.
    pub(super) modules: Region,
}
impl<'a> LoadPlan<'a> {
    pub(super) fn new(
        images: BootImages<'a>,
        loader: Region,
        minimum: usize,
    ) -> Result<Self, BootError> {
        let kernel = &images.kernel;
        let root = &images.root;
        if kernel.start < platform::KERNEL_VA_START
            || kernel.end > platform::KERNEL_VA_END
            || !kernel.start.is_multiple_of(platform::BLOCK_SIZE)
        {
            return Err(layout::Error::KernelWindow.into());
        }
        let task_layout = InitialTaskLayout::new(
            root.start as u64..root.end as u64,
            images.dtb.bytes().len() as u64,
        )
        .ok_or(layout::Error::RootLayout)?;
        let image_pages: usize = root.segments().map(|s| (s.end - s.va) / PAGE).sum();
        if image_pages + task_layout.metadata_pages() > MAX_USER_PAGES {
            return Err(layout::Error::RootLayout.into());
        }
        let header_size = 2 * core::mem::size_of::<u32>();
        if root
            .headers
            .len()
            .checked_add(header_size)
            .is_none_or(|size| size > PAGE)
        {
            return Err(layout::Error::HeaderPage.into());
        }
        let kernel_size = kernel.end - kernel.start;
        let dtb_size = images.dtb.bytes().len();
        let root_offset = page_up(
            kernel_size
                .checked_add(dtb_size)
                .ok_or(layout::Error::Overflow)?,
        )
        .map_err(|_| layout::Error::Overflow)?;
        let headers_offset = root_offset
            .checked_add(root.end - root.start)
            .ok_or(layout::Error::Overflow)?;
        let headers_end = headers_offset
            .checked_add(PAGE)
            .ok_or(layout::Error::Overflow)?;
        let modules_len = page_up(images.raw.len()).map_err(|_| layout::Error::Overflow)?;
        let total = headers_end
            .checked_add(modules_len)
            .ok_or(layout::Error::Overflow)?;
        let region = layout::allocate(
            Region::new(platform::RAM_START, platform::RAM_END - platform::RAM_START)?,
            &[
                Region::new(
                    platform::RAM_START,
                    platform::FIRMWARE_END - platform::RAM_START,
                )?,
                loader,
            ],
            minimum,
            total,
            platform::BLOCK_SIZE,
        )?;
        Ok(Self {
            kernel: ImageMapping::new(Region::new(region.start(), kernel_size)?, kernel.start)?,
            root: ImageMapping::new(
                Region::new(region.start() + root_offset, root.end - root.start)?,
                root.start,
            )?,
            dtb: Region::new(region.start() + kernel_size, dtb_size)?,
            headers: Region::new(region.start() + headers_offset, PAGE)?,
            modules: Region::new(region.start() + headers_end, modules_len)?,
            images,
        })
    }
}

#[cfg(test)]
#[path = "../../tests/image_cases.rs"]
mod tests;
