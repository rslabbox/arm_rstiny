use super::mem::Aligned4K;
use crate::config::BOOT_STACK_SIZE;
use aarch64_cpu::asm::barrier;
use aarch64_cpu::registers::*;
use memory_addr::{PhysAddr, pa};
use page_table_entry::{GenericPTE, MappingFlags, aarch64::A64PTE};

#[unsafe(link_section = ".bss.stack")]
static mut BOOT_STACK: [u8; BOOT_STACK_SIZE] = [0; BOOT_STACK_SIZE];

#[unsafe(link_section = ".data")]
static mut BOOT_PT_L0: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

#[unsafe(link_section = ".data")]
static mut BOOT_PT_L1: Aligned4K<[A64PTE; 512]> = Aligned4K::new([A64PTE::empty(); 512]);

/// Swtich current exception level to EL1.
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

/// Configures and enables the MMU on the current CPU.
///
/// It first sets `MAIR_EL1`, `TCR_EL1`, `TTBR0_EL1`, `TTBR1_EL1` registers to
/// the conventional values, and then enables the MMU and caches by setting
/// `SCTLR_EL1`.
///
/// # Safety
///
/// This function is unsafe as it changes the address translation configuration.
pub unsafe fn init_mmu(root_paddr: PhysAddr) {
    use page_table_entry::aarch64::MemAttr;

    MAIR_EL1.set(MemAttr::MAIR_VALUE);

    // Enable TTBR0 and TTBR1 walks, page size = 4K, vaddr size = 48 bits, paddr size = 48 bits.
    let tcr_flags0 = TCR_EL1::EPD0::EnableTTBR0Walks
        + TCR_EL1::TG0::KiB_4
        + TCR_EL1::SH0::Inner
        + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::T0SZ.val(16);
    let tcr_flags1 = TCR_EL1::EPD1::EnableTTBR1Walks
        + TCR_EL1::TG1::KiB_4
        + TCR_EL1::SH1::Inner
        + TCR_EL1::ORGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::IRGN1::WriteBack_ReadAlloc_WriteAlloc_Cacheable
        + TCR_EL1::T1SZ.val(16);
    TCR_EL1.write(TCR_EL1::IPS::Bits_48 + tcr_flags0 + tcr_flags1);
    barrier::isb(barrier::SY);

    // Set both TTBR0 and TTBR1
    let root_paddr = root_paddr.as_usize() as u64;
    TTBR0_EL1.set(root_paddr);
    TTBR1_EL1.set(root_paddr);

    // Flush the entire TLB
    crate::arch::flush_tlb(None);

    // Enable the MMU and turn on I-cache and D-cache
    SCTLR_EL1.modify(SCTLR_EL1::M::Enable + SCTLR_EL1::C::Cacheable + SCTLR_EL1::I::Cacheable);
    barrier::isb(barrier::SY);
}

unsafe fn init_boot_page_table() {
    unsafe {
        // 0x0000_0000_0000 ~ 0x0080_0000_0000, table
        BOOT_PT_L0[0] = A64PTE::new_table(pa!(&raw mut BOOT_PT_L1 as usize));
        
        // Map low memory (0-4GB) for kernel and normal devices
        // 0x0000_0000_0000..0x0000_4000_0000, 1G block, normal memory
        BOOT_PT_L1[0] = A64PTE::new_page(
            pa!(0x0),
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::EXECUTE,
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
            MappingFlags::READ | MappingFlags::WRITE | MappingFlags::DEVICE,
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

/// Enable FP/SIMD instructions by setting the `FPEN` field in `CPACR_EL1`.
pub fn enable_fp() {
    CPACR_EL1.write(CPACR_EL1::FPEN::TrapNothing);
    barrier::isb(barrier::SY);
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
unsafe extern "C" fn _start() -> ! {
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
        mrs     x19, mpidr_el1
        and     x19, x19, #0xffffff     // get current CPU id
        mov     x20, x0                 // save DTB pointer

        adrp    x8, {boot_stack}        // setup boot stack
        add     x8, x8, {boot_stack_size}
        mov     sp, x8

        bl      {switch_to_el1}         // switch to EL1
        bl      {enable_fp}             // enable fp/neon
        bl      {init_boot_page_table}
        adrp    x0, {boot_pt}
        bl      {init_mmu}            // setup MMU

        mov     x0, x19                 // call_main(cpu_id, dtb)
        mov     x1, x20
        ldr     x8, ={rust_main}
        blr     x8
        b      .",
        switch_to_el1 = sym switch_to_el1,
        init_mmu = sym init_mmu,
        enable_fp = sym enable_fp,
        init_boot_page_table = sym init_boot_page_table,
        boot_stack = sym BOOT_STACK,
        boot_pt = sym BOOT_PT_L0,
        boot_stack_size = const BOOT_STACK_SIZE,
        rust_main = sym crate::rust_main,
    )
}
