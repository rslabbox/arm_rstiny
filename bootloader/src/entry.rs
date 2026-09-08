//! Raw AArch64 entry, boot stack setup and BSS initialization.
use aarch64_cpu::registers::{CurrentEL, Readable, SCTLR_EL1};
use core::{arch::naked_asm, ptr::addr_of};

unsafe extern "C" {
    static __bss_start: u8;
    static __bss_end: u8;
}

/// Raw entry: mask exceptions, select the boot CPU, establish SP, enter Rust.
///
/// Runs before a stack or initialized BSS is available. Only the QEMU boot CPU
/// (all MPIDR affinity fields zero) may use the single boot stack. Other CPUs
/// park before using SP. Rust checks the entry state after the stack is ready.
///
/// # Safety
/// The image must be loaded at its linked physical address in writable RAM and
/// entered in privileged AArch64 execution with system-register access allowed.
/// The supported boot context is EL1 with MMU and caches disabled. Firmware must
/// guarantee that code and the linked boot stack are accessible before any Rust
/// validation runs; those checks cannot protect the first stack access. Firmware
/// must not enter the boot CPU here again once initialization has begun.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        // No exception vectors are installed yet: mask debug exceptions,
        // SError, IRQ and FIQ before inspecting the boot environment.
        "msr     daifset, #0xf",

        // Select the fixed platform's boot CPU using Aff3:Aff2:Aff1:Aff0.
        // Exclude MPIDR's non-affinity flags; other CPUs must not share our SP.
        "mrs     x9, mpidr_el1",
        "ldr     x10, =0xff00ffffff",
        "tst     x9, x10",
        "b.ne    2f",

        // Select SP_ELx (SP_EL1 on the supported path) and install the
        // linker-reserved, 16-byte-aligned stack.
        // The absolute address is valid because the image is loaded at its
        // linked physical address. No ordinary Rust call is safe before this.
        "msr     spsel, #1",
        "ldr     x9, =__stack_top",
        "mov     sp, x9",

        // Tail-enter Rust: validate the entry state, then clear BSS.
        "b       {main}",

        // Other CPUs remain stackless. WFE can wake
        // spuriously, so always loop instead of falling through into Rust.
        "2:",
        "wfe",
        "b       2b",

        main = sym start,
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
            addr_of!(__bss_start).cast_mut(),
            0,
            addr_of!(__bss_end) as usize - addr_of!(__bss_start) as usize,
        );
    }
}

/// Check the firmware contract before touching BSS or initializing the console.
/// The stack is already active, so this is not a pre-stack safety check.
fn check_boot_context() {
    // Check EL first: do not inspect EL1 control state as if it described the
    // active environment when firmware entered at a different exception level.
    if CurrentEL.read(CurrentEL::EL) != 1
        || SCTLR_EL1.is_set(SCTLR_EL1::M)
        || SCTLR_EL1.is_set(SCTLR_EL1::C)
        || SCTLR_EL1.is_set(SCTLR_EL1::I)
    {
        // No global state or UART access is safe to assume on this path.
        crate::console::bootloader_halt();
    }
}

extern "C" fn start() -> ! {
    check_boot_context();
    // SAFETY: Only the boot CPU reaches here, before any Rust global state is used.
    unsafe { clear_bss() };
    crate::boot_main()
}
