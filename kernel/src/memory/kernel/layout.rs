//! Describe what the kernel maps, independently of AArch64 page-table levels.
use super::MapError;
use crate::memory::address::{KernelImage, phys_to_virt};
use crate::{
    arch::kernel::boot::BootInfo,
    config::{self, MemFlags, PAGE_SIZE},
};
use core::ptr::addr_of;
use memory_addr::{PhysAddr, VirtAddr};

#[derive(Clone, Copy)]
pub(super) struct Region {
    pub virtual_start: VirtAddr,
    pub physical_start: PhysAddr,
    pub size: usize,
    pub flags: MemFlags,
}

pub(super) struct KernelLayout {
    image: [Region; 4],
    loaded: Region,
    devices: [Region; 3],
}
impl KernelLayout {
    pub fn from_boot(info: BootInfo) -> Result<Self, MapError> {
        unsafe extern "C" {
            static etext: u8;
            static erodata: u8;
            static stack_guard: u8;
        }
        let mapping = KernelImage::from_boot(info).map_err(|_| MapError::InvalidRange)?;
        let start = mapping.virtual_start().as_usize();
        let text_end = addr_of!(etext) as usize;
        let rodata_end = addr_of!(erodata) as usize;
        let guard = addr_of!(stack_guard) as usize;
        let end = mapping.virtual_end().as_usize();
        let boundaries = [start, text_end, rodata_end, guard, guard + PAGE_SIZE, end];
        if boundaries.iter().any(|address| address % PAGE_SIZE != 0)
            || boundaries.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(MapError::InvalidRange);
        }
        let physical_end = mapping.physical_end().as_usize();
        let loaded_end = info
            .image_end
            .checked_add(PAGE_SIZE)
            .ok_or(MapError::InvalidRange)?;
        if info.kernel_physical < config::RAM_START
            || info.kernel_physical % PAGE_SIZE != 0
            || physical_end > info.image_start
            || info.image_start >= info.image_end
            || loaded_end > config::RAM_END
        {
            return Err(MapError::InvalidRange);
        }
        let region = |begin: usize, end: usize, flags| Region {
            virtual_start: VirtAddr::from_usize(begin),
            physical_start: mapping
                .to_phys(VirtAddr::from_usize(begin))
                .expect("validated image region"),
            size: end - begin,
            flags,
        };
        let image = [
            region(start, text_end, MemFlags::READ | MemFlags::EXECUTE),
            region(text_end, rodata_end, MemFlags::READ),
            region(rodata_end, guard, MemFlags::READ | MemFlags::WRITE),
            region(guard + PAGE_SIZE, end, MemFlags::READ | MemFlags::WRITE),
        ];
        // Loader metadata, DTB, root image and preserved program-header page.
        // This is a discovered physical extent, not part of the kernel image VA.
        let loaded = Region {
            virtual_start: phys_to_virt(PhysAddr::from_usize(physical_end))
                .expect("direct-map address"),
            physical_start: PhysAddr::from_usize(physical_end),
            size: loaded_end - physical_end,
            flags: MemFlags::READ | MemFlags::WRITE,
        };
        let device = |physical, size| Region {
            virtual_start: phys_to_virt(PhysAddr::from_usize(physical))
                .expect("direct-map address"),
            physical_start: PhysAddr::from_usize(physical),
            size,
            flags: MemFlags::READ | MemFlags::WRITE | MemFlags::DEVICE,
        };
        Ok(Self {
            image,
            loaded,
            devices: [
                device(config::GICD_BASE, config::GICD_SIZE),
                device(config::GICR_BASE, config::GICR_SIZE),
                // Panic output needs UART even when ordinary logging is disabled.
                device(config::UART_BASE, PAGE_SIZE),
            ],
        })
    }
    pub fn image_regions(&self) -> impl Iterator<Item = Region> + '_ {
        self.image.iter().copied()
    }
    pub fn physical_regions(&self) -> impl Iterator<Item = Region> + '_ {
        self.image
            .iter()
            .map(|region| Region {
                virtual_start: phys_to_virt(region.physical_start).expect("image direct alias"),
                flags: region.flags & !MemFlags::EXECUTE,
                ..*region
            })
            .chain(core::iter::once(self.loaded))
    }
    pub fn device_regions(&self) -> impl Iterator<Item = Region> + '_ {
        self.devices.iter().copied()
    }
}
