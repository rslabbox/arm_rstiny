//! QEMU virt GICv3 and the non-secure physical timer, single CPU only.
use crate::config::{GICD_BASE, GICR_BASE, phys_to_virt};
use aarch64_cpu::{asm::barrier, registers::*};
use arm_gic_driver::{
    IntId, VirtAddr,
    v3::{CpuInterface, Gic, Trigger},
};
use core::cell::UnsafeCell;

const TIMER_IRQ: IntId = IntId::ppi(14); // Non-secure physical timer, INTID 30.
struct Controller {
    gic: Gic,
    cpu: CpuInterface,
}
struct LocalController(UnsafeCell<Option<Controller>>);
// SAFETY: single CPU; initialization and IRQ handling run with IRQs masked.
// No reference to the controller escapes either operation.
unsafe impl Sync for LocalController {}
static CONTROLLER: LocalController = LocalController(UnsafeCell::new(None));

pub fn masked() -> bool {
    DAIF.is_set(DAIF::I)
}
pub fn now() -> u64 {
    CNTPCT_EL0.get()
}
pub fn frequency() -> u64 {
    CNTFRQ_EL0.get()
}
pub fn rearm() {
    CNTP_TVAL_EL0.set((frequency() / 100).max(1));
    CNTP_CTL_EL0.set(1);
    barrier::isb(barrier::SY);
}
pub fn init() {
    assert!(masked());
    // SAFETY: exclusive boot-time access on the sole CPU.
    let slot = unsafe { &mut *CONTROLLER.0.get() };
    assert!(slot.is_none(), "GIC already initialized");
    CNTP_CTL_EL0.set(0);
    // SAFETY: one controller, permanently mapped Device memory at EL1 only.
    let mut gic = unsafe {
        Gic::new(
            VirtAddr::new(phys_to_virt(GICD_BASE)),
            VirtAddr::new(phys_to_virt(GICR_BASE)),
        )
    };
    gic.init();
    let mut cpu = gic.cpu_interface();
    cpu.init_current_cpu().expect("GICv3 CPU initialization");
    cpu.set_eoi_mode(false); // EOI drops priority and deactivates the interrupt.
    cpu.set_cfg(TIMER_IRQ, Trigger::Level);
    cpu.set_priority(TIMER_IRQ, 0x80);
    cpu.set_pending(TIMER_IRQ, false);
    cpu.set_irq_enable(TIMER_IRQ, true);
    *slot = Some(Controller { gic, cpu });
    CNTKCTL_EL1.set(0); // EL0 cannot reprogram timers or enable its own event stream.
    CPACR_EL1.set(0); // FP/SIMD remains trapped until its context is supported.
    rearm();
}
/// Quiesce the source before EOI; special/spurious IDs require no EOI.
pub fn handle() -> bool {
    assert!(masked());
    // SAFETY: non-reentrant IRQ handling on the sole CPU.
    let controller = unsafe { &mut *CONTROLLER.0.get() }
        .as_mut()
        .expect("IRQ before GIC initialization");
    let id = controller.cpu.ack1();
    if id.is_special() {
        return false;
    }
    if id == TIMER_IRQ {
        rearm();
    } else {
        // No user IRQ delivery yet: mask unexpected sources to avoid a storm.
        controller.gic.set_irq_enable(id, false);
    }
    barrier::dsb(barrier::SY);
    controller.cpu.eoi1(id);
    id == TIMER_IRQ
}
