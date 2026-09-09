//! Consume a validated plan to load images and construct kernel handoff metadata.
use super::{Handoff, LoadPlan};
use crate::image::elf::PAGE;

impl LoadPlan<'_> {
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
            self.copy_modules();
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
            modules: self.modules.start(),
            modules_size: self.modules.size(),
        }
    }
    /// The whole archive, byte for byte, so the root task can parse its boot
    /// modules; the kernel publishes the range as read-only Frame capabilities.
    fn copy_modules(&self) {
        // SAFETY: Called by load with exclusive destination ownership; the
        // region was sized to the page-rounded archive length during planning.
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.images.raw.as_ptr(),
                self.modules.start() as *mut u8,
                self.images.raw.len(),
            );
            let padding = self.modules.size() - self.images.raw.len();
            core::ptr::write_bytes(
                (self.modules.start() + self.images.raw.len()) as *mut u8,
                0,
                padding,
            );
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
