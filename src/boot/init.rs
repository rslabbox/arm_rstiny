//! Early boot initialization code.

use memory_addr::pa;
use page_table_entry::{GenericPTE, MappingFlags, aarch64::A64PTE};

use crate::config::kernel::{BOOT_STACK_SIZE, SECONDARY_STACK_SIZE, TINYENV_SMP};
use crate::mm::Aligned4K;

#[unsafe(link_section = ".bss.stack")]
pub static mut BOOT_STACK: [u8; BOOT_STACK_SIZE] = [0; BOOT_STACK_SIZE];

/// Secondary CPU boot stacks (one for each secondary CPU).
#[unsafe(link_section = ".bss.stack")]
pub static mut SECONDARY_STACKS: [[u8; SECONDARY_STACK_SIZE]; TINYENV_SMP - 1] =
    [[0; SECONDARY_STACK_SIZE]; TINYENV_SMP - 1];

#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L0: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

#[unsafe(link_section = ".data")]
static mut BOOT_PT_L1: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// Initialize boot page table.
///
/// This creates a simple identity mapping for the kernel and device memory.
/// The page table will be replaced by a more sophisticated one later.
///
/// # Safety
///
/// This function is unsafe as it modifies global static variables.
#[unsafe(no_mangle)]
pub unsafe fn init_boot_page_table() {
    unsafe {
        // 0x0000_0000_0000 ~ 0x0080_0000_0000, table
        BOOT_PT_L0[0] = A64PTE::new_table(pa!(&raw mut BOOT_PT_L1 as usize));

        // Map low memory (0-4GB) for kernel and normal devices
        // 0x0000_0000_0000..0x0000_4000_0000, 1G block, normal memory
        BOOT_PT_L1[0] = A64PTE::new_page(
            pa!(0x0),
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::DEVICE,
            true,
        );
        // 1G block, normal memory
        BOOT_PT_L1[1] = A64PTE::new_page(
            pa!(0x40000000),
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
            true,
        );
        // 1G block, normal memory
        BOOT_PT_L1[2] = A64PTE::new_page(
            pa!(0x80000000),
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
            true,
        );
        // 1G block, device memory. From 0xfb000000
        BOOT_PT_L1[3] = A64PTE::new_page(
            pa!(0xc0000000),
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
            true,
        );

        // Map PCIe ECAM configuration space at 0x0a_40c00000 (42GB)
        // This is required for PCI device enumeration
        BOOT_PT_L1[41] = A64PTE::new_page(
            pa!(0x0a_40000000), // 42GB, 1G block aligned
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::DEVICE,
            true,
        );
    }
}
