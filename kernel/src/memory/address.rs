//! Checked address arithmetic, not a page-table lookup or proof of accessibility.
//! The image's PA is discovered from the loader's live mapping at boot.
use crate::{
    arch::kernel::boot,
    config::{KERNEL_OFFSET, PA_MAX_BITS, PAGE_SIZE, RAM_END, RAM_START},
};
use core::ptr::addr_of;
use memory_addr::{PhysAddr, VirtAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddressError {
    Uninitialized,
    InvalidRange,
    OutsideImage,
    OutsideDirectMap,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct KernelImage {
    virtual_start: VirtAddr,
    physical_start: PhysAddr,
    size: usize,
}
impl KernelImage {
    pub fn from_boot(info: boot::BootInfo) -> Result<Self, AddressError> {
        unsafe extern "C" {
            static skernel: u8;
            static ekernel: u8;
        }
        if info.kernel_physical == 0 {
            return Err(AddressError::Uninitialized);
        }
        let virtual_start = addr_of!(skernel) as usize;
        let size = (addr_of!(ekernel) as usize)
            .checked_sub(virtual_start)
            .ok_or(AddressError::InvalidRange)?;
        if size == 0
            || size % PAGE_SIZE != 0
            || info.kernel_physical % PAGE_SIZE != 0
            || info.kernel_physical < RAM_START
            || info
                .kernel_physical
                .checked_add(size)
                .is_none_or(|end| end > RAM_END)
        {
            return Err(AddressError::InvalidRange);
        }
        Ok(Self {
            virtual_start: VirtAddr::from_usize(virtual_start),
            physical_start: PhysAddr::from_usize(info.kernel_physical),
            size,
        })
    }
    pub fn virtual_start(self) -> VirtAddr {
        self.virtual_start
    }
    pub fn virtual_end(self) -> VirtAddr {
        VirtAddr::from_usize(self.virtual_start.as_usize() + self.size)
    }
    pub fn physical_end(self) -> PhysAddr {
        PhysAddr::from_usize(self.physical_start.as_usize() + self.size)
    }
    pub fn to_phys(self, va: VirtAddr) -> Result<PhysAddr, AddressError> {
        let offset = va
            .as_usize()
            .checked_sub(self.virtual_start.as_usize())
            .filter(|offset| *offset < self.size)
            .ok_or(AddressError::OutsideImage)?;
        Ok(PhysAddr::from_usize(
            self.physical_start.as_usize() + offset,
        ))
    }
    pub fn to_virt(self, pa: PhysAddr) -> Result<VirtAddr, AddressError> {
        let offset = pa
            .as_usize()
            .checked_sub(self.physical_start.as_usize())
            .filter(|offset| *offset < self.size)
            .ok_or(AddressError::OutsideImage)?;
        Ok(VirtAddr::from_usize(self.virtual_start.as_usize() + offset))
    }
}
pub(crate) fn kernel_image() -> Result<KernelImage, AddressError> {
    KernelImage::from_boot(boot::information())
}

/// Choose the direct-map alias. A valid arithmetic result need not be mapped.
pub(crate) fn phys_to_virt(pa: PhysAddr) -> Result<VirtAddr, AddressError> {
    if pa.as_usize() >= 1usize << PA_MAX_BITS {
        return Err(AddressError::OutsideDirectMap);
    }
    Ok(VirtAddr::from_usize(KERNEL_OFFSET + pa.as_usize()))
}
/// Decode a direct-map alias; never accept low/user or kernel-image addresses.
pub(crate) fn direct_to_phys(va: VirtAddr) -> Result<PhysAddr, AddressError> {
    va.as_usize()
        .checked_sub(KERNEL_OFFSET)
        .filter(|pa| *pa < 1usize << PA_MAX_BITS)
        .map(PhysAddr::from_usize)
        .ok_or(AddressError::OutsideDirectMap)
}
/// Convert a known kernel image/direct-map VA without consulting page tables.
/// Kernel image conversion requires validated, published boot information.
pub(crate) fn virt_to_phys(va: VirtAddr) -> Result<PhysAddr, AddressError> {
    if let Ok(pa) = direct_to_phys(va) {
        return Ok(pa);
    }
    kernel_image()?.to_phys(va)
}
