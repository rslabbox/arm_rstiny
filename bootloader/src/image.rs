//! Parse images, validate a placement plan, then explicitly commit physical writes.
use crate::{
    archive::{ArchiveError, BootArchive},
    boot_info::Handoff,
    device_tree::{self, DeviceTree},
    elf::{Elf, PAGE, page_up},
    layout::{self, ImageMapping, Region},
    platform,
};
use core::{fmt, ptr::addr_of};
use kernel_abi::{InitialTaskLayout, MAX_USER_PAGES};

unsafe extern "C" {
    static __archive_start: u8;
    static __archive_end: u8;
    static __loader_start: u8;
    static __loader_end: u8;
}
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
struct BootImages<'a> {
    kernel: Elf<'a>,
    root: Elf<'a>,
    dtb: DeviceTree<'a>,
}
impl<'a> BootImages<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, BootError> {
        let archive = BootArchive::parse(bytes).map_err(BootError::Archive)?;
        Ok(Self {
            kernel: Elf::parse(archive.kernel()).map_err(BootError::KernelElf)?,
            root: Elf::parse(archive.rootserver()).map_err(BootError::RootElf)?,
            dtb: DeviceTree::parse(archive.device_tree()).map_err(BootError::DeviceTree)?,
        })
    }
}
/// Only planning can construct this object. It owns every validated destination
/// and consumes itself when committing, preserving validate-before-write ordering.
pub(crate) struct LoadPlan<'a> {
    images: BootImages<'a>,
    kernel: ImageMapping,
    root: ImageMapping,
    dtb: Region,
    headers: Region,
}
impl<'a> LoadPlan<'a> {
    fn new(images: BootImages<'a>, loader: Region, minimum: usize) -> Result<Self, BootError> {
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
        let total = headers_offset
            .checked_add(PAGE)
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
            images,
        })
    }
    /// # Safety
    /// Call once during single-CPU boot with IRQs masked and MMU/caches off.
    /// The planned RAM must be exclusively owned and physically accessible.
    pub unsafe fn load(self) -> Handoff {
        // SAFETY: Planning proved every range fits free RAM and excludes the
        // entire loader (including archive/stack/tables). ELF source extents and
        // the retained-header page size were validated before any write occurs.
        unsafe {
            self.images.kernel.load(self.kernel.physical().start());
            core::ptr::copy_nonoverlapping(
                self.images.dtb.bytes().as_ptr(),
                self.dtb.start() as *mut u8,
                self.dtb.size(),
            );
            self.images.root.load(self.root.physical().start());
            self.write_root_headers();
        }
        Handoff {
            image_start: self.root.physical().start(),
            image_end: self.root.physical().end(),
            offset: self.root.physical().start() - self.root.virtual_start(),
            root_entry: self.images.root.entry,
            dtb: self.dtb.start(),
            dtb_size: self.dtb.size(),
            kernel_entry: self.images.kernel.entry,
            kernel_mapping: self.kernel,
        }
    }
    unsafe fn write_root_headers(&self) {
        const PHDR_SIZE: u32 = rstiny_elf::PROGRAM_HEADER_SIZE as u32;
        let destination = self.headers.start();
        // SAFETY: Called by load with exclusive destination ownership; new()
        // checked that the two u32 fields and original PHDR table fit one page.
        unsafe {
            core::ptr::write_bytes(destination as *mut u8, 0, PAGE);
            (destination as *mut u32).write(self.images.root.count as u32);
            (destination as *mut u32).add(1).write(PHDR_SIZE);
            core::ptr::copy_nonoverlapping(
                self.images.root.headers.as_ptr(),
                (destination as *mut u32).add(2).cast(),
                self.images.root.headers.len(),
            );
        }
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
