//! Kernel entry point with Linux image header and assembly startup code.

use aarch64_cpu::asm::barrier;
use aarch64_cpu::registers::*;

use crate::config::kernel::PHYS_VIRT_OFFSET;

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
pub unsafe extern "C" fn _start() -> ! {
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
unsafe extern "C" fn _start_primary() -> ! {
    core::arch::naked_asm!("
        // dump x0/x1 to PL011 (0x0900_0000)
        mov     x21, x0
        mov     x22, x1
        ldr     x2, ={uart_base}

        // print x0 (hex)
        mov     x6, #60
    1:
        lsr     x7, x21, x6
        and     x7, x7, #0xf
        add     x7, x7, #0x30
        cmp     x7, #0x39
        ble     2f
        add     x7, x7, #0x27
    2:
    3:
        ldr     w4, [x2, #0x18]
        tst     w4, #0x20
        b.ne    3b
        strb    w7, [x2]
        subs    x6, x6, #4
        b.ge    1b

        // print space
        mov     w7, #0x20
    4:
        ldr     w4, [x2, #0x18]
        tst     w4, #0x20
        b.ne    4b
        strb    w7, [x2]

        // print x1 (hex)
        mov     x6, #60
    5:
        lsr     x7, x22, x6
        and     x7, x7, #0xf
        add     x7, x7, #0x30
        cmp     x7, #0x39
        ble     6f
        add     x7, x7, #0x27
    6:
    7:
        ldr     w4, [x2, #0x18]
        tst     w4, #0x20
        b.ne    7b
        strb    w7, [x2]
        subs    x6, x6, #4
        b.ge    5b

        // newline
        mov     w7, #0x0a
    8:
        ldr     w4, [x2, #0x18]
        tst     w4, #0x20
        b.ne    8b
        strb    w7, [x2]

        mov     x0, x21
        mov     x1, x22

        mrs     x19, mpidr_el1
        and     x19, x19, #0xffffff     // get current CPU id
        mov     x20, x0                 // save DTB pointer

        adrp    x8, {boot_stack}        // setup boot stack
        add     x8, x8, {boot_stack_size}
        mov     sp, x8

        bl      {switch_to_el1}         // switch to EL1
        bl      {enable_fp}             // enable fp/neon
        bl      {init_boot_page_table}

        ldr x10, =0x09000000
        mov w11, #'A'
        str w11, [x10]

        adrp    x0, {boot_pt}
        bl      {init_mmu}            // setup MMU

        ldr x10, =0x09000000
        mov w11, #'B'
        str w11, [x10]

        mov     x8, {phys_virt_offset}  // set SP to the high address
        add     sp, sp, x8

        mov     x0, x19                 // call_main(cpu_id, dtb)
        mov     x1, x20
        ldr     x8, ={rust_main}
        blr     x8
        b      .",
        switch_to_el1 = sym switch_to_el1,
        init_mmu = sym super::mmu::init_mmu,
        enable_fp = sym enable_fp,
        init_boot_page_table = sym super::init::init_boot_page_table,
        boot_stack = sym super::init::BOOT_STACK,
        boot_pt = sym super::init::BOOT_PT_L0,
        phys_virt_offset = const PHYS_VIRT_OFFSET,
        boot_stack_size = const crate::config::kernel::BOOT_STACK_SIZE,
        rust_main = sym super::rust_main,
        uart_base = const 0x0900_0000,
    )
}

/// Secondary CPU entry point.
///
/// Called by PSCI cpu_on with cpu_id in x0.
/// Each secondary CPU has its own stack allocated in SECONDARY_STACKS.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
pub unsafe extern "C" fn _start_secondary() -> ! {
    core::arch::naked_asm!("
        // x0 = cpu_id (passed from PSCI cpu_on)
        mov     x19, x0                     // save cpu_id

        // Calculate stack address for this CPU
        // stack_addr = SECONDARY_STACKS + (cpu_id - 1) * SECONDARY_STACK_SIZE + SECONDARY_STACK_SIZE
        adrp    x8, {secondary_stacks}
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
        adrp    x0, {boot_pt}
        bl      {init_mmu}                  // setup MMU

        // Switch to virtual address space
        mov     x8, {phys_virt_offset}
        add     sp, sp, x8

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
        init_mmu = sym super::mmu::init_mmu,
        phys_virt_offset = const PHYS_VIRT_OFFSET,
        rust_main_secondary = sym super::rust_main_secondary,
    )
}
