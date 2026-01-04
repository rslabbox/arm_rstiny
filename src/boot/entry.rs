//! Kernel entry point with Linux image header and assembly startup code.

use aarch64_cpu::asm::barrier;
use aarch64_cpu::registers::*;

use crate::config::kernel::TINYENV_KIMAGE_VADDR;

/// Enable FP/SIMD instructions by setting the `FPEN` field in `CPACR_EL1`.
pub fn enable_fp() {
    CPACR_EL1.write(CPACR_EL1::FPEN::TrapNothing);
    barrier::isb(barrier::SY);
}

/// Switch current exception level to EL1.
///
/// It usually used in the system booting process, where the startup code is
/// running in EL2 or EL3. Besides, the stack is not available and the MMU is
/// not enabled.
///
/// # Safety
///
/// This function is unsafe as it changes the CPU mode.
pub unsafe fn switch_to_el1() {
    SPSel.write(SPSel::SP::ELx);
    SP_EL0.set(0);
    let current_el = CurrentEL.read(CurrentEL::EL);
    if current_el >= 2 {
        if current_el == 3 {
            // Set EL2 to 64bit and enable the HVC instruction.
            SCR_EL3.write(
                SCR_EL3::NS::NonSecure + SCR_EL3::HCE::HvcEnabled + SCR_EL3::RW::NextELIsAarch64,
            );
            // Set the return address and exception level.
            SPSR_EL3.write(
                SPSR_EL3::M::EL1h
                    + SPSR_EL3::D::Masked
                    + SPSR_EL3::A::Masked
                    + SPSR_EL3::I::Masked
                    + SPSR_EL3::F::Masked,
            );
            ELR_EL3.set(LR.get());
        }
        // Disable EL1 timer traps and the timer offset.
        CNTHCTL_EL2.modify(CNTHCTL_EL2::EL1PCEN::SET + CNTHCTL_EL2::EL1PCTEN::SET);
        CNTVOFF_EL2.set(0);
        // Set EL1 to 64bit.
        HCR_EL2.write(HCR_EL2::RW::EL1IsAarch64);
        // Set the return address and exception level.
        SPSR_EL2.write(
            SPSR_EL2::M::EL1h
                + SPSR_EL2::D::Masked
                + SPSR_EL2::A::Masked
                + SPSR_EL2::I::Masked
                + SPSR_EL2::F::Masked,
        );
        SP_EL1.set(SP.get());
        ELR_EL2.set(LR.get());
        aarch64_cpu::asm::eret();
    }
}

/// Kernel entry point with Linux image header.
///
/// Some bootloaders require this header to be present at the beginning of the
/// kernel image.
///
/// Documentation: <https://docs.kernel.org/arch/arm64/booting.html>
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
pub unsafe extern "C" fn _start() {
    const FLAG_LE: usize = 0b0;
    const FLAG_PAGE_SIZE_4K: usize = 0b10;
    const FLAG_ANY_MEM: usize = 0b1000;
    // PC = bootloader load address
    // X0 = dtb
    core::arch::naked_asm!("
        add     x13, x18, #0x16     // 'MZ' magic
        b       {entry}             // Branch to kernel start, magic

        .quad   0                   // Image load offset from start of RAM, little-endian
        .quad   _ekernel - _start   // Effective size of kernel image, little-endian
        .quad   {flags}             // Kernel flags, little-endian
        .quad   0                   // reserved
        .quad   0                   // reserved
        .quad   0                   // reserved
        .ascii  \"ARM\\x64\"        // Magic number
        .long   0                   // reserved (used for PE COFF offset)",
        flags = const FLAG_LE | FLAG_PAGE_SIZE_4K | FLAG_ANY_MEM,
        entry = sym _start_primary,
    )
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start_primary() {
    // Linux-style page table setup using 2MB blocks for kernel mapping
    // This allows the kernel to be loaded at any 2MB aligned physical address.
    //
    // Page table structure:
    // TTBR0 (identity mapping): L0 -> L1 -> L2 (2MB blocks)
    // TTBR1 (kernel mapping):   L0 -> L1 -> L2 (2MB blocks)
    //
    // The key insight is that with 2MB blocks:
    // - VA[20:0] passes through unchanged
    // - VA[29:21] indexes into L2
    // - VA[38:30] indexes into L1
    // - VA[47:39] indexes into L0
    //
    // For KIMAGE_VADDR = 0xffff_0000_8000_0000 mapping to phys_base:
    // - We need L1[2] to point to L2 table (not a 1GB block)
    // - L2 entries map 2MB blocks with correct physical addresses
    core::arch::naked_asm!("
        mrs     x19, mpidr_el1
        and     x19, x19, #0xffffff     // get current CPU id
        mov     x20, x0                 // save DTB pointer

        // ============================================================
        // Position-independent: Calculate actual physical base address
        // ============================================================
        adrp    x21, _skernel           // x21 = physical base (2MB aligned load address)
        
        // Calculate VA to PA offset: x22 = KIMAGE_VADDR - phys_base
        ldr     x22, ={kimage_vaddr}
        sub     x22, x22, x21           // x22 = va_to_pa_offset

        // Setup boot stack using position-independent addressing
        adrp    x8, {boot_stack}
        add     x8, x8, {boot_stack_size}
        mov     sp, x8

        bl      {switch_to_el1}
        bl      {enable_fp}

        // ============================================================
        // Setup page tables using Rust function
        // ============================================================
        mov     x0, x21                     // phys_base
        bl      {init_boot_page_table}

        // ============================================================
        // Get physical addresses of page tables for MMU enable
        // ============================================================
        adrp    x10, {boot_pt_l0_ident}     // x10 = L0_IDENT PA
        adrp    x12, {boot_pt_l0}           // x12 = L0 PA

        // ============================================================
        // Enable MMU
        // ============================================================
        mov     x0, x10                     // TTBR0
        mov     x1, x12                     // TTBR1
        bl      {init_mmu}

        // ============================================================
        // Switch to virtual address space
        // ============================================================
        add     sp, sp, x22                 // SP -> VA

        mov     x0, x19                     // cpu_id
        mov     x1, x20                     // dtb
        mov     x2, x21                     // kernel_phys_base
        ldr     x8, ={rust_main}
        blr     x8
        b      .",
        switch_to_el1 = sym switch_to_el1,
        init_mmu = sym super::mmu::init_mmu,
        init_boot_page_table = sym super::init::init_boot_page_table,
        enable_fp = sym enable_fp,
        boot_stack = sym super::init::BOOT_STACK,
        boot_pt_l0 = sym super::init::BOOT_PT_L0,
        boot_pt_l0_ident = sym super::init::BOOT_PT_L0_IDENT,
        kimage_vaddr = const TINYENV_KIMAGE_VADDR,
        boot_stack_size = const crate::config::kernel::BOOT_STACK_SIZE,
        rust_main = sym super::rust_main,
    )
}

/// Secondary CPU entry point.
///
/// Called by PSCI cpu_on with cpu_id in x0.
/// Each secondary CPU has its own stack allocated in SECONDARY_STACKS.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
pub unsafe extern "C" fn _start_secondary() {
    core::arch::naked_asm!("
        // x0 = cpu_id (passed from PSCI cpu_on)
        mov     x19, x0                     // save cpu_id

        // Calculate physical base for offset computation
        adrp    x21, _skernel               // x21 = KERNEL_PHYS_BASE (physical)
        ldr     x22, ={kimage_vaddr}
        sub     x22, x22, x21               // x22 = va_to_pa_offset

        // Calculate stack address for this CPU (using physical addresses)
        adrp    x8, {secondary_stacks}      // physical address
        add     x8, x8, :lo12:{secondary_stacks}
        sub     x9, x19, #1                 // index = cpu_id - 1
        mov     x10, {stack_size}
        mul     x9, x9, x10
        add     x8, x8, x9
        add     x8, x8, x10                 // point to stack top
        mov     sp, x8

        bl      {switch_to_el1}             // switch to EL1
        bl      {enable_fp}                 // enable fp/neon

        // Secondary CPUs reuse the same page table set up by primary
        // adrp gives physical address when MMU is off
        adrp    x0, {boot_pt_ident}         // TTBR0: identity mapping (physical addr)
        adrp    x1, {boot_pt}               // TTBR1: kernel mapping (physical addr)
        bl      {init_mmu}                  // setup MMU

        // Switch SP to virtual address
        add     sp, sp, x22

        // Call rust_main_secondary(cpu_id)
        mov     x0, x19
        ldr     x8, ={rust_main_secondary}
        blr     x8
        b       .",
        secondary_stacks = sym super::init::SECONDARY_STACKS,
        stack_size = const crate::config::kernel::SECONDARY_STACK_SIZE,
        switch_to_el1 = sym switch_to_el1,
        enable_fp = sym enable_fp,
        boot_pt = sym super::init::BOOT_PT_L0,
        boot_pt_ident = sym super::init::BOOT_PT_L0_IDENT,
        init_mmu = sym super::mmu::init_mmu,
        kimage_vaddr = const TINYENV_KIMAGE_VADDR,
        rust_main_secondary = sym super::rust_main_secondary,
    )
}
