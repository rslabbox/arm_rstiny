use arm_gic::IntId;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use memory_addr::VirtAddr;
pub mod boot;

pub mod device;
pub mod exception;
pub mod mem;

/// Flushes the TLB.
///
/// If `vaddr` is [`None`], flushes the entire TLB. Otherwise, flushes the TLB
/// entry that maps the given virtual address.
#[inline]
pub fn flush_tlb(vaddr: Option<VirtAddr>) {
    if let Some(vaddr) = vaddr {
        const VA_MASK: usize = (1 << 44) - 1; // VA[55:12] => bits[43:0]
        let operand = (vaddr.as_usize() >> 12) & VA_MASK;

        unsafe {
            // TLB Invalidate by VA, All ASID, EL1, Inner Shareable

            asm!("tlbi vaae1is, {}; dsb sy; isb", in(reg) operand)
        }
    } else {
        // flush the entire TLB
        unsafe {
            // TLB Invalidate by VMID, All at stage 1, EL1
            asm!("tlbi vmalle1; dsb sy; isb")
        }
    }
}

// Setup timer interrupt handler
const PERIODIC_INTERVAL_NANOS: u64 =
    device::generic_timer::NANOS_PER_SEC / crate::config::TICKS_PER_SEC as u64;

static NEXT_DEADLINE: AtomicU64 = AtomicU64::new(0);
pub fn update_timer() {
    let current_ns = device::generic_timer::ticks_to_nanos(device::generic_timer::current_ticks());
    let mut deadline = NEXT_DEADLINE.load(Ordering::Relaxed);
    if current_ns >= deadline {
        deadline = current_ns + PERIODIC_INTERVAL_NANOS;
    }
    // Here we can set the next timer deadline, for example, 1 second later
    let next_deadline_ns = deadline + device::generic_timer::NANOS_PER_SEC;
    NEXT_DEADLINE.store(next_deadline_ns, Ordering::Relaxed);
    device::generic_timer::set_oneshot_timer(next_deadline_ns);
}

pub fn arch_init() {
    exception::init_trap();
    mem::clear_bss();
    device::generic_timer::init_early();
    device::psci::init("hvc");
    device::gicv3::irq_init();

    // Enable Timer irq
    device::gicv3::irqset_register(IntId::ppi(14), |_| update_timer());
    device::generic_timer::enable_irqs(IntId::ppi(14));
}
