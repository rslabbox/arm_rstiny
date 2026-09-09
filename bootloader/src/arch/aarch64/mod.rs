//! EL1 boot environment for the supported AArch64 platform.
mod entry;
mod handoff;
mod mmu;
pub(crate) use handoff::enter;

/// Park without touching global state, including before BSS initialization.
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn bootloader_halt() -> ! {
    loop {
        // SAFETY: Boot runs in privileged AArch64 with interrupts masked.
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack));
        }
    }
}
