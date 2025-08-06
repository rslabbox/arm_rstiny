use core::arch::asm;

#[inline]
pub fn flush_tlb_all() {
    unsafe { asm!("tlbi vmalle1; dsb sy; isb") };
}
