//! User address spaces own their mappings, frames and private page tables.
mod frame;
mod space;
pub use frame::available as available_frames;
pub use frame::{finish_boot, prepare_boot};
pub use space::AddressSpace;
pub const PAGE_SIZE: usize = 4096;
pub const USER_START: usize = PAGE_SIZE;
pub const USER_END: usize = kernel_abi::USER_ADDRESS_LIMIT as usize;
pub const MAX_PAGES: usize = kernel_abi::MAX_USER_PAGES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Error {
    InvalidArgument = kernel_abi::INVALID_ARGUMENT,
    NoMemory = kernel_abi::NO_MEMORY,
    NotMapped = kernel_abi::NOT_MAPPED,
    AlreadyMapped = kernel_abi::ALREADY_MAPPED,
    Permission = kernel_abi::PERMISSION_DENIED,
}

pub fn validate_permissions(flags: u64) -> Result<(), Error> {
    if matches!(flags, 1 | 3 | 5) {
        Ok(())
    } else {
        Err(Error::InvalidArgument)
    }
}

pub fn sync_translations() {
    aarch64_cpu::asm::barrier::dsb(aarch64_cpu::asm::barrier::ISH);
    crate::arch::instructions::flush_tlb_all();
}

pub fn activate(root: usize) {
    use aarch64_cpu::registers::{TTBR0_EL1, Writeable};
    aarch64_cpu::asm::barrier::dsb(aarch64_cpu::asm::barrier::ISH);
    TTBR0_EL1.set(root as u64);
    sync_translations();
}

pub fn activate_kernel() {
    activate(crate::arch::boot::kernel_root());
}

pub fn sync_code(address: usize, size: usize) {
    use aarch64_cpu::asm::barrier;
    let ctr: u64;
    // SAFETY: EL1 cache maintenance uses an owned, identity-mapped frame range.
    unsafe {
        core::arch::asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nomem, nostack));
        let line = 4usize << ((ctr >> 16) & 15);
        for p in (address & !(line - 1)..address + size).step_by(line) {
            core::arch::asm!("dc cvau, {0}", in(reg) p, options(nostack));
        }
        barrier::dsb(barrier::ISH);
        core::arch::asm!("ic iallu", options(nostack));
    }
    barrier::dsb(barrier::ISH);
    barrier::isb(barrier::SY);
}
