//! seL4 elfloader handoff, BSS initialization and entry into kernel startup.
use aarch64_cpu::{asm::barrier, registers::*};
use core::ptr::{addr_of, addr_of_mut};

use crate::config::{PAGE_SIZE, RAM_END, RAM_START};

// seL4 ARM loader passes x0..x5 and enters an EL1 kernel with MMU/caches on.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct BootInfo {
    pub image_start: usize,
    pub image_end: usize,
    pub phys_virt_offset: usize,
    pub entry: usize,
    pub dtb: usize,
    pub dtb_size: usize,
    pub kernel_physical: usize,
}
#[unsafe(no_mangle)]
static mut LOADER_BOOT_INFO: BootInfo = BootInfo {
    image_start: 0,
    image_end: 0,
    phys_virt_offset: 0,
    entry: 0,
    dtb: 0,
    dtb_size: 0,
    kernel_physical: 0,
};
pub fn information() -> BootInfo {
    // SAFETY: initialized once before kernel startup, then immutable on this CPU.
    unsafe { core::ptr::addr_of!(LOADER_BOOT_INFO).read() }
}

/// Raw entry: mask exceptions, select the boot CPU, establish SP, enter Rust.
///
/// The seL4 ARM elfloader enters here at EL1 with MMU and caches already
/// enabled and passes the initial image layout in x0..x5. Runs before a stack
/// or initialized BSS is available; only the boot CPU (all MPIDR affinity
/// fields zero) may use the single boot stack. x0..x5 must reach `start_rust`
/// unchanged, so this sequence touches only scratch registers (x9/x10) and SP.
///
/// # Safety
/// Entered once by the elfloader with x0..x5 holding the loader handoff values;
/// `start_rust` revalidates the EL/MMU state and every loader-provided address.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        // No exception vectors are installed yet: mask debug exceptions,
        // SError, IRQ and FIQ before inspecting the boot environment.
        "msr     daifset, #0xf",

        // Select the boot CPU using Aff3:Aff2:Aff1:Aff0, excluding MPIDR's
        // non-affinity flags (U, MT); other CPUs must not share our SP.
        "mrs     x9, mpidr_el1",
        "ldr     x10, =0xff00ffffff",
        "tst     x9, x10",
        "b.ne    2f",

        // Select SP_ELx (SP_EL1 on the supported path) and install the
        // linker-reserved boot stack. x0..x5 stay untouched so the loader's
        // image-layout arguments reach Rust unchanged.
        "msr     spsel, #1",
        "ldr     x9, =boot_stack_top",
        "mov     sp, x9",

        // Tail-enter Rust: validate the handoff state, then clear BSS.
        "b       {start}",

        // Other CPUs remain stackless. WFE can wake spuriously, so always
        // loop instead of falling through into Rust.
        "2:",
        "wfe",
        "b       2b",

        start = sym start_rust,
    );
}

/// Zero the linker-defined BSS before initializing Rust global state.
///
/// # Safety
/// Call only once on the boot CPU, before accessing BSS-backed globals.
/// The BSS range must be writable and disjoint from the active stack.
unsafe fn clear_bss() {
    // SAFETY: The linker reserves BSS separately from the boot stack, and the
    // caller guarantees exclusive access before global initialization begins.
    unsafe {
        core::ptr::write_bytes(
            addr_of!(sbss).cast_mut(),
            0,
            addr_of!(ebss) as usize - addr_of!(sbss) as usize,
        );
    }
}

extern "C" fn start_rust(
    image_start: usize,
    image_end: usize,
    phys_virt_offset: usize,
    entry: usize,
    dtb: usize,
    dtb_size: usize,
) -> ! {
    if CurrentEL.read(CurrentEL::EL) != 1
        || !SCTLR_EL1.is_set(SCTLR_EL1::M)
        || !SCTLR_EL1.is_set(SCTLR_EL1::C)
        || !SCTLR_EL1.is_set(SCTLR_EL1::I)
    {
        crate::utils::halt();
    }
    VBAR_EL1.set(exception_vector_base as *const () as u64);
    barrier::isb(barrier::SY);
    // SAFETY: Only the boot CPU reaches here, before BSS-backed globals are used.
    unsafe { clear_bss() };
    // Resolve the kernel image through the loader's live translation regime.
    // This keeps x0..x5 unchanged and requires no fixed physical load address.
    let kernel_virtual = addr_of!(skernel) as usize;
    let Some(kernel_physical) = crate::arch::machine::mmu::translate_current(
        memory_addr::VirtAddr::from_usize(kernel_virtual),
    ) else {
        crate::utils::halt()
    };
    let kernel_physical = kernel_physical.as_usize();
    let kernel_size = addr_of!(ekernel) as usize - kernel_virtual;
    let Some(kernel_end) = kernel_physical.checked_add(kernel_size) else {
        crate::utils::halt()
    };
    let info = BootInfo {
        image_start,
        image_end,
        phys_virt_offset,
        entry,
        dtb,
        dtb_size,
        kernel_physical,
    };
    let Some(user_start) = image_start.checked_sub(phys_virt_offset) else {
        crate::utils::halt()
    };
    let Some(user_end) = image_end.checked_sub(phys_virt_offset) else {
        crate::utils::halt()
    };
    if kernel_abi::InitialTaskLayout::new(user_start as u64..user_end as u64, dtb_size as u64)
        .is_none()
        || !(user_start..user_end).contains(&entry)
    {
        crate::utils::halt();
    }
    // Check ranges before using any loader-controlled address in a page table.
    if kernel_physical < RAM_START
        || !kernel_physical.is_multiple_of(PAGE_SIZE)
        || kernel_end > RAM_END
        || image_start < kernel_end
        || image_start >= image_end
        || image_end > RAM_END - PAGE_SIZE
        || !image_start.is_multiple_of(PAGE_SIZE)
        || !image_end.is_multiple_of(PAGE_SIZE)
        || dtb < kernel_end
        || dtb_size < 40
        || dtb_size > kernel_abi::MAX_DTB_SIZE as usize
        || dtb
            .checked_add(dtb_size)
            .is_none_or(|end| end > image_start)
    {
        crate::utils::halt();
    }
    // SAFETY: IRQs remain masked and no other CPU executes kernel code.
    unsafe {
        addr_of_mut!(LOADER_BOOT_INFO).write(info);
    }
    boot_main()
}

unsafe extern "C" {
    fn exception_vector_base();
    static mut sbss: u8;
    static mut ebss: u8;
    static skernel: u8;
    static ekernel: u8;
}

#[unsafe(no_mangle)]
extern "C" fn boot_main() -> ! {
    // SAFETY: validated loader handoff on the boot CPU, before global startup.
    unsafe {
        crate::memory::kernel::init();
    }
    crate::rust_main()
}
