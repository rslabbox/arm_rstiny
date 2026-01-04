//! Early boot initialization code.

use memory_addr::pa;
use page_table_entry::{GenericPTE, MappingFlags, aarch64::A64PTE};

use crate::config::kernel::{BOOT_STACK_SIZE, TINYENV_KIMAGE_VADDR, SECONDARY_STACK_SIZE, TINYENV_SMP};
use crate::mm::Aligned4K;

#[unsafe(link_section = ".bss.stack")]
pub static mut BOOT_STACK: [u8; BOOT_STACK_SIZE] = [0; BOOT_STACK_SIZE];

/// Secondary CPU boot stacks (one for each secondary CPU).
#[unsafe(link_section = ".bss.stack")]
pub static mut SECONDARY_STACKS: [[u8; SECONDARY_STACK_SIZE]; TINYENV_SMP - 1] =
    [[0; SECONDARY_STACK_SIZE]; TINYENV_SMP - 1];

/// L0 page table for TTBR1 (kernel high address mapping).
#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L0: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// L1 page table for TTBR1 (kernel high address mapping).
#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L1: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// L2 page table for TTBR1 kernel mapping (2MB blocks).
/// This is used to map the kernel at precise 2MB aligned addresses.
#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L2: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// L0 page table for TTBR0 (identity mapping).
#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L0_IDENT: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// L1 page table for TTBR0 (identity mapping).
#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L1_IDENT: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// L2 page table for TTBR0 identity mapping (2MB blocks).
#[unsafe(link_section = ".data")]
pub static mut BOOT_PT_L2_IDENT: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// 1GB block size for page table mapping.
const GB_BLOCK_SIZE: usize = 1 << 30; // 1GB = 0x4000_0000

/// Initialize boot page table with position-independent physical address.
///
/// This creates:
/// 1. Identity mapping (PA -> PA) in TTBR0 for the kernel's physical location
/// 2. High address mapping (VA -> PA) in TTBR1 for the kernel virtual addresses
///
/// # Arguments
/// * `phys_base` - The actual physical address where the kernel was loaded (2MB aligned)
///
/// # Safety
///
/// This function is unsafe as it modifies global static variables.
/// Must be called before MMU is enabled, accessing page tables via physical addresses.
#[unsafe(no_mangle)]
pub unsafe fn init_boot_page_table(phys_base: usize) {
    // Calculate the 1GB block index for the kernel's physical location
    let kernel_gb_index = phys_base / GB_BLOCK_SIZE;
    let kernel_gb_base = kernel_gb_index * GB_BLOCK_SIZE;

    // Calculate the 1GB block index for kernel virtual address
    // KIMAGE_VADDR = 0xffff_0000_8000_0000
    // For TTBR1, we use the lower 48 bits: 0x0000_8000_0000
    // L0 index = bits[47:39] = 0
    // L1 index = bits[38:30] = 2 (for 0x8000_0000)
    let kimage_offset = TINYENV_KIMAGE_VADDR & 0x0000_FFFF_FFFF_FFFF; // Lower 48 bits
    let kimage_l1_index = (kimage_offset >> 30) & 0x1FF;

    unsafe {
        // Get physical addresses of all page tables and convert to pointers
        // This is critical: MMU is off, we must access via physical addresses!
        // Since we are running with MMU off, and code is position independent (using adrp),
        // taking the address of a static variable returns its physical address.
        let pt_l0_ident_pa = &raw mut BOOT_PT_L0_IDENT as usize;
        let pt_l1_ident_pa = &raw mut BOOT_PT_L1_IDENT as usize;
        let pt_l2_ident_pa = &raw mut BOOT_PT_L2_IDENT as usize;
        let pt_l0_pa = &raw mut BOOT_PT_L0 as usize;
        let pt_l1_pa = &raw mut BOOT_PT_L1 as usize;
        let pt_l2_pa = &raw mut BOOT_PT_L2 as usize;

        // Convert to mutable pointers for writing
        let pt_l0_ident = pt_l0_ident_pa as *mut A64PTE;
        let pt_l1_ident = pt_l1_ident_pa as *mut A64PTE;
        let pt_l2_ident = pt_l2_ident_pa as *mut A64PTE;
        let pt_l0 = pt_l0_pa as *mut A64PTE;
        let pt_l1 = pt_l1_pa as *mut A64PTE;
        let pt_l2 = pt_l2_pa as *mut A64PTE;

        // ============================================================
        // Setup TTBR0: Identity mapping for physical addresses
        // This allows code to continue running after MMU is enabled
        // ============================================================

        // L0[0] -> L1_IDENT table (covers 0x0000_0000_0000 ~ 0x0080_0000_0000)
        pt_l0_ident.add(0).write(A64PTE::new_table(pa!(pt_l1_ident_pa)));

        // Map the 1GB block containing the kernel (identity: PA -> PA)
        // We use L2 table for precise 2MB mapping to match kernel load address alignment
        pt_l1_ident.add(kernel_gb_index).write(A64PTE::new_table(pa!(pt_l2_ident_pa)));

        // Fill L2_IDENT with 512 consecutive 2MB blocks covering the 1GB region
        // Each entry maps: (kernel_gb_index * 1GB) + (i * 2MB)
        let mut pa_start = kernel_gb_base;
        for i in 0..512 {
            pt_l2_ident.add(i).write(A64PTE::new_page(
                pa!(pa_start),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                true, // 2MB block
            ));
            pa_start += 0x200000; // 2MB
        }

        // Map additional blocks for device memory if needed
        // For QEMU virt: UART at 0x0900_0000, GIC at 0x0800_0000 (all in first 1GB)
        // For OrangePi5: devices at 0xfe00_0000+ (4th 1GB block, index 3)
        if kernel_gb_index != 0 {
            pt_l1_ident.add(0).write(A64PTE::new_page(
                pa!(0x0),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                true,
            ));
        }
        if kernel_gb_index != 3 {
            pt_l1_ident.add(3).write(A64PTE::new_page(
                pa!(0xc0000000),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::DEVICE,
                true,
            ));
        }

        // ============================================================
        // Setup TTBR1: High address mapping for kernel virtual addresses
        // Maps 0xffff_0000_xxxx_xxxx -> physical addresses
        // ============================================================

        // L0[0] -> L1 table
        pt_l0.add(0).write(A64PTE::new_table(pa!(pt_l1_pa)));

        // Map the kernel image: KIMAGE_VADDR -> phys_base
        // The L1 index for 0xffff_0000_8000_0000 is 2
        // We use L2 table for precise 2MB mapping
        pt_l1.add(kimage_l1_index).write(A64PTE::new_table(pa!(pt_l2_pa)));

        // Calculate kimage_l2_index
        let kimage_l2_index = (kimage_offset >> 21) & 0x1FF;
        
        // Calculate start_pa = phys_base - (kimage_l2_index * 2MB)
        // This ensures that VA corresponding to kimage_l2_index maps to phys_base
        let start_pa = phys_base.wrapping_sub(kimage_l2_index * 0x200000);
        
        let mut pa_current = start_pa;
        for i in 0..512 {
            pt_l2.add(i).write(A64PTE::new_page(
                pa!(pa_current),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                true, // 2MB block
            ));
            pa_current += 0x200000; // 2MB
        }

        // Map low memory for device access through high addresses
        // 0xffff_0000_0000_0000 -> 0x0 (first 1GB, devices for QEMU)
        if kimage_l1_index != 0 {
            pt_l1.add(0).write(A64PTE::new_page(
                pa!(0x0),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                true,
            ));
        }

        // Map 0xffff_0000_4000_0000 -> 0x4000_0000 (second 1GB)
        if kimage_l1_index != 1 {
            pt_l1.add(1).write(A64PTE::new_page(
                pa!(0x40000000),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
                true,
            ));
        }

        // Map device memory: 0xffff_0000_c000_0000 -> 0xc000_0000 (OrangePi5 devices)
        if kimage_l1_index != 3 {
            pt_l1.add(3).write(A64PTE::new_page(
                pa!(0xc0000000),
                MappingFlags::READ | MappingFlags::WRITE | MappingFlags::DEVICE,
                true,
            ));
        }
    }
}

/// Get the physical address of the TTBR0 page table (identity mapping).
#[inline]
#[allow(dead_code)]
pub fn boot_pt_ttbr0_paddr() -> usize {
    &raw mut BOOT_PT_L0_IDENT as usize
}

/// Get the physical address of the TTBR1 page table (kernel mapping).
#[inline]
#[allow(dead_code)]
pub fn boot_pt_ttbr1_paddr() -> usize {
    &raw mut BOOT_PT_L0 as usize
}
