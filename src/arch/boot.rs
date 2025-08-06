use core::arch::{asm, naked_asm};

// Boot page table allocation
#[unsafe(link_section = ".data.boot_page_table")]
static mut BOOT_PAGE_TABLE: [u64; 512] = [0; 512];

// External symbols from linker script
unsafe extern "C" {
    static boot_stack_top: u8;
    static sbss: u8;
    static ebss: u8;
}

// External function from main.rs
unsafe extern "C" {
    fn rust_main() -> !;
}

// Page table entry flags
const PTE_VALID: u64 = 1 << 0;
const PTE_BLOCK: u64 = 0 << 1;
const PTE_AF: u64 = 1 << 10; // Access flag
const PTE_ATTR_NORMAL: u64 = 1 << 2; // Memory attribute index
const PTE_ATTR_DEVICE: u64 = 0 << 2;

/// Boot entry point - this is the _start function called by the bootloader
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
pub unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        "mov x8, #97",
        "mov x9, #0x09000000", //串口地址，需要变化
        "str x8, [x9]",
        // Set up stack pointer
        "adr x0, {boot_stack_top}",
        "mov sp, x0",

        // Call boot sequence functions
        "bl {switch_to_el1}",

        "bl {clear_bss}",

        "bl {init_boot_page_table}",

        "bl {init_exception_vector}",

        "bl {enable_mmu}",

        "bl {rust_main}",

        // Should never reach here
        "1: wfe",
        "b 1b",

        boot_stack_top = sym boot_stack_top,
        switch_to_el1 = sym switch_to_el1,
        clear_bss = sym clear_bss,
        init_boot_page_table = sym init_boot_page_table,
        init_exception_vector = sym init_exception_vector,
        enable_mmu = sym enable_mmu,
        rust_main = sym rust_main
    );
}

/// Switch from current exception level to EL1
#[unsafe(no_mangle)]
pub unsafe extern "C" fn switch_to_el1() {
    unsafe {
        asm!(
            // Check current exception level
            "mrs x0, CurrentEL",
            "and x0, x0, #0xC",
            "cmp x0, #0x8", // EL2
            "b.eq 2f",
            "cmp x0, #0xC", // EL3
            "b.eq 1f",
            "b 3f", // Already in EL1 or EL0
            // From EL3 to EL2
            "1:",
            "mov x0, #0x5b1", // RES1 bits
            "msr scr_el3, x0",
            "mov x0, #0x3c9", // EL2h, disable interrupts
            "msr spsr_el3, x0",
            "adr x0, 2f",
            "msr elr_el3, x0",
            "eret",
            // From EL2 to EL1
            "2:",
            "mov x0, #0x80000000", // HCR_EL2.RW = 1 (AArch64)
            "msr hcr_el2, x0",
            "mov x0, #0x3c5", // EL1h, disable interrupts
            "msr spsr_el2, x0",
            "adr x0, 3f",
            "msr elr_el2, x0",
            "eret",
            // Now in EL1
            "3:",
            "nop",
            options(preserves_flags)
        );
    }
}

/// Clear the BSS section to zero
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clear_bss() {
    unsafe {
        let start = &sbss as *const u8 as *mut u8;
        let end = &ebss as *const u8 as *mut u8;
        let size = end.offset_from(start) as usize;

        core::ptr::write_bytes(start, 0, size);
    }
}

/// Initialize the boot page table for identity mapping
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_boot_page_table() {
    unsafe {
        // Clear the page table
        #[allow(static_mut_refs)]
        for entry in BOOT_PAGE_TABLE.iter_mut() {
            *entry = 0;
        }

        // Create identity mapping for the first 1GB (0x0000_0000 - 0x3FFF_FFFF)
        // This covers our kernel space and device memory
        BOOT_PAGE_TABLE[0] = 0x0000_0000 | PTE_VALID | PTE_BLOCK | PTE_AF | PTE_ATTR_NORMAL;

        // Map device memory region (0x4000_0000 - 0x7FFF_FFFF)
        BOOT_PAGE_TABLE[1] = 0x4000_0000 | PTE_VALID | PTE_BLOCK | PTE_AF | PTE_ATTR_DEVICE;

        // Additional mappings can be added here as needed
    }
}

/// Initialize exception vector table
#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_exception_vector() {
    unsafe {
        asm!(
            "adr x0, exception_vector_base",
            "msr vbar_el1, x0",
            "isb",
            options(preserves_flags)
        );
    }
}

/// Enable MMU with basic configuration
#[unsafe(no_mangle)]
pub unsafe extern "C" fn enable_mmu() {
    unsafe {
        asm!(
            // Set TTBR0_EL1 to point to our page table
            "adr x0, {boot_page_table}",
            "msr ttbr0_el1, x0",

            // Configure TCR_EL1 for 39-bit VA space
            "mov x0, #25",                    // T0SZ = 25 (39-bit VA space)
            "orr x0, x0, #0x100",            // IRGN0 = 1 (Inner WB RW-Allocate)
            "orr x0, x0, #0x400",            // ORGN0 = 1 (Outer WB RW-Allocate)
            "orr x0, x0, #0x3000",           // SH0 = 3 (Inner Shareable)
            // TG0 = 0 (4KB granule) is already 0, no need to set
            "msr tcr_el1, x0",

            // Configure MAIR_EL1 (Memory Attribute Indirection Register)
            "mov x0, #0x44",                 // Normal memory, Inner/Outer WB
            "msr mair_el1, x0",

            // Enable MMU in SCTLR_EL1
            "mrs x0, sctlr_el1",
            "orr x0, x0, #1",                // Enable MMU (M bit)
            "msr sctlr_el1, x0",
            "isb",

            boot_page_table = sym BOOT_PAGE_TABLE,
            options(preserves_flags)
        );
    }
}
