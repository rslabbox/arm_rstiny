//! Install the final kernel mappings under the elfloader's live MMU regime.
use aarch64_cpu::{asm::barrier, registers::*};
use memory_addr::PhysAddr;

/// # Safety
/// Both roots must be initialized, aligned, stable and exclusively owned by the
/// single boot CPU. The high root must preserve the current code, stack, globals
/// and page-table storage. The low root must be empty. IRQs must remain masked.
pub(crate) unsafe fn install_kernel_roots(high: PhysAddr, low: PhysAddr) {
    // Keep elfloader's MAIR indices: Attr0=Device-nGnRnE, Attr4=normal WB.
    // Changing attributes while its mappings are still live would be unsafe.
    barrier::dsb(barrier::SY);
    TTBR1_EL1.set(high.as_usize() as u64);
    TTBR0_EL1.set(low.as_usize() as u64);
    barrier::isb(barrier::SY);
    super::instructions::flush_tlb_all();
    TCR_EL1.modify(TCR_EL1::SH0::Inner + TCR_EL1::SH1::Inner);
    barrier::isb(barrier::SY);
    SCTLR_EL1.modify(SCTLR_EL1::SA::Enable + SCTLR_EL1::SA0::Enable + SCTLR_EL1::WXN::Enable);
    barrier::isb(barrier::SY);
}

/// Query the current EL1 stage-1 translation regime, including loader mappings.
/// Clobbers PAR_EL1; IRQs must be masked so the query/result form one operation.
/// This checks translation, not whether a later Rust dereference is valid.
pub(crate) fn translate_current(va: memory_addr::VirtAddr) -> Option<PhysAddr> {
    assert!(super::instructions::irq_masked());
    // SAFETY: privileged translation query at EL1; no memory is dereferenced.
    unsafe {
        core::arch::asm!("at s1e1r, {va}", va = in(reg) va.as_usize(), options(nostack));
    }
    barrier::isb(barrier::SY);
    let result = PAR_EL1.extract();
    if result.is_set(PAR_EL1::F) {
        return None;
    }
    Some(PhysAddr::from_usize(
        ((result.read(PAR_EL1::PA) as usize) << 12) | (va.as_usize() & 4095),
    ))
}
