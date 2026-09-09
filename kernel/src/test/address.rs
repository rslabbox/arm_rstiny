//! Arithmetic, software translation and current hardware translations agree.
use crate::{
    arch::{
        kernel::{boot, vspace::paging::MapError},
        machine::mmu,
    },
    config::{MemFlags, PA_MAX_BITS, PAGE_SIZE, RAM_START},
    memory::{
        self, AddressSpace, Error,
        address::{self, KernelImage},
    },
};
use core::ptr::addr_of;
use memory_addr::{PhysAddr, VirtAddr};

pub fn run() {
    kernel();
    user();
}
fn kernel() {
    unsafe extern "C" {
        static etext: u8;
        static __heap_start: u8;
        static stack_guard: u8;
    }
    let image = address::kernel_image().unwrap();
    let va = VirtAddr::from_usize(image.virtual_start().as_usize() + 123);
    let actual = address::virt_to_phys(va).unwrap();
    assert_eq!(actual.as_usize(), boot::information().kernel_physical + 123);
    assert_eq!(image.to_virt(actual).unwrap(), va);
    for offset in [2 * 1024 * 1024, 64 * 1024 * 1024] {
        let relocated = KernelImage::from_boot(boot::BootInfo {
            kernel_physical: RAM_START + offset,
            ..boot::information()
        })
        .unwrap();
        let pa = relocated.to_phys(va).unwrap();
        assert_eq!(pa.as_usize(), RAM_START + offset + 123);
        assert_eq!(relocated.to_virt(pa).unwrap(), va);
    }
    assert!(KernelImage::from_boot(boot::BootInfo::default()).is_err());
    assert!(image.to_phys(image.virtual_end()).is_err());
    assert!(image.to_virt(image.physical_end()).is_err());
    assert!(address::virt_to_phys(image.virtual_end()).is_err());
    assert!(address::virt_to_phys(VirtAddr::from_usize(123)).is_err());
    assert!(address::phys_to_virt(PhysAddr::from_usize(1usize << PA_MAX_BITS)).is_err());
    assert!(address::direct_to_phys(va).is_err());

    for va in [
        va,
        VirtAddr::from_usize(addr_of!(etext) as usize + 31),
        VirtAddr::from_usize(addr_of!(__heap_start) as usize + 4095),
    ] {
        let pa = address::virt_to_phys(va).unwrap();
        let direct = address::phys_to_virt(pa).unwrap();
        let image_mapping = memory::kernel::translate(va).unwrap();
        let direct_mapping = memory::kernel::translate(direct).unwrap();
        assert_eq!(image_mapping.physical, pa);
        assert_eq!(direct_mapping.physical, pa);
        assert_eq!(address::direct_to_phys(direct).unwrap(), pa);
        assert_eq!(mmu::translate_current(va), Some(pa));
        assert_eq!(mmu::translate_current(direct), Some(pa));
        assert_eq!(image_mapping.page_size, PAGE_SIZE);
        assert_eq!(
            direct_mapping.flags,
            image_mapping.flags & !MemFlags::EXECUTE
        );
    }
    let guard = VirtAddr::from_usize(addr_of!(stack_guard) as usize + 3);
    // Arithmetic is defined for a hole; lookup must still reject it.
    let physical = image.to_phys(guard).unwrap();
    for va in [guard, address::phys_to_virt(physical).unwrap()] {
        assert_eq!(memory::kernel::translate(va), Err(MapError::NotMapped));
        assert!(mmu::translate_current(va).is_none());
    }
    assert_eq!(
        memory::kernel::translate(image.virtual_end()),
        Err(MapError::NotMapped)
    );
}
fn user() {
    let before = memory::available_frames();
    {
        let mut first = AddressSpace::new().unwrap();
        let mut second = AddressSpace::new().unwrap();
        let base = 0x1000000;
        first.map(base, 2 * PAGE_SIZE, 3, false).unwrap();
        second.map(base, PAGE_SIZE, 3, false).unwrap();
        let va = VirtAddr::from_usize(base + 37);
        let mapping = first.translate(va).unwrap();
        assert_eq!(
            mapping.physical.as_usize(),
            first.frame_at(base).unwrap().physical() + 37
        );
        assert_ne!(mapping.physical, second.translate(va).unwrap().physical);
        assert_eq!(
            mapping.flags,
            MemFlags::READ | MemFlags::WRITE | MemFlags::USER
        );
        first.write(base + PAGE_SIZE - 3, b"cross-page").unwrap();
        let mut bytes = [0; 10];
        first.read(base + PAGE_SIZE - 3, &mut bytes).unwrap();
        assert_eq!(&bytes, b"cross-page");
        first.protect(base, PAGE_SIZE, 5).unwrap();
        assert_eq!(
            first.translate(va).unwrap().flags,
            MemFlags::READ | MemFlags::EXECUTE | MemFlags::USER
        );
        first.unmap(base, PAGE_SIZE).unwrap();
        assert_eq!(first.translate(va), Err(Error::NotMapped));
        assert!(second.translate(va).is_ok());
        assert_eq!(
            first.translate(VirtAddr::from_usize(0)),
            Err(Error::InvalidArgument)
        );
    }
    assert_eq!(memory::available_frames(), before);
}
