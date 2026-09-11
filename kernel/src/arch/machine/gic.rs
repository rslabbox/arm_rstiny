//! GICv3 controller operations for the single CPU, independent of IRQ consumers.
use crate::memory::address::phys_to_virt;
use crate::{
    config::{GICD_BASE, GICR_BASE},
    utils::single_core::SingleCore,
};
use aarch64_cpu::asm::barrier;
pub(crate) use arm_gic_driver::{IntId as IrqId, v3::Trigger};
use arm_gic_driver::{
    VirtAddr,
    v3::{CpuInterface, Gic},
};

struct Controller {
    gic: Gic,
    cpu: CpuInterface,
    active: Option<IrqId>,
}
static CONTROLLER: SingleCore<Option<Controller>> = SingleCore::new(None);
fn with_controller<T>(operation: impl FnOnce(&mut Controller) -> T) -> T {
    let mut slot = CONTROLLER.borrow_mut();
    operation(slot.as_mut().expect("GIC not initialized"))
}
impl Controller {
    fn validate(&self, irq: IrqId) {
        // This fixed platform supports ordinary SGIs/PPIs/SPIs, not LPIs/ESPIs.
        assert!(
            irq.to_u32() < 1020 && irq.to_u32() < self.gic.max_intid(),
            "unsupported GIC interrupt ID"
        );
    }
}

/// Initialize the distributor and local CPU interface with all sources disabled.
/// Requires the permanent Device mappings and masked local IRQs.
pub(crate) fn init() {
    let mut slot = CONTROLLER.borrow_mut();
    assert!(slot.is_none(), "GIC already initialized");
    // SAFETY: single owner of permanently mapped controller registers at EL1.
    let mut gic = unsafe {
        Gic::new(
            VirtAddr::new(
                phys_to_virt(memory_addr::PhysAddr::from_usize(GICD_BASE))
                    .expect("direct-map address")
                    .as_usize(),
            ),
            VirtAddr::new(
                phys_to_virt(memory_addr::PhysAddr::from_usize(GICR_BASE))
                    .expect("direct-map address")
                    .as_usize(),
            ),
        )
    };
    gic.init();
    let mut cpu = gic.cpu_interface();
    cpu.init_current_cpu().expect("GICv3 CPU initialization");
    // Split EOI (EOImode=1, docs/irq.md §5): delivery drops priority via EOIR
    // only and leaves the line active; the driver's Ack deactivates via DIR.
    // Combined EOI would deactivate at delivery time, re-arming an uncleared
    // level source before its driver has even seen the notification.
    cpu.set_eoi_mode(true);
    *slot = Some(Controller {
        gic,
        cpu,
        active: None,
    });
}

pub(crate) fn set_enable(irq: IrqId, enabled: bool) {
    with_controller(|controller| {
        controller.validate(irq);
        if irq.is_private() {
            controller.cpu.set_irq_enable(irq, enabled);
        } else {
            controller.gic.set_irq_enable(irq, enabled);
        }
    });
}

pub(crate) fn set_trigger(irq: IrqId, trigger: Trigger) {
    with_controller(|controller| {
        controller.validate(irq);
        assert!(!irq.is_sgi(), "SGI trigger is fixed");
        if irq.is_private() {
            controller.cpu.set_cfg(irq, trigger);
        } else {
            controller.gic.set_cfg(irq, trigger);
        }
    });
}

pub(crate) fn set_priority(irq: IrqId, priority: u8) {
    with_controller(|controller| {
        controller.validate(irq);
        if irq.is_private() {
            controller.cpu.set_priority(irq, priority);
        } else {
            controller.gic.set_priority(irq, priority);
        }
    });
}

pub(crate) fn set_pending(irq: IrqId, pending: bool) {
    with_controller(|controller| {
        controller.validate(irq);
        if irq.is_private() {
            controller.cpu.set_pending(irq, pending);
        } else {
            controller.gic.set_pending(irq, pending);
        }
    });
}

/// One hardware claim, owned until explicit completion; never held across park.
#[must_use = "complete the claimed interrupt after servicing its source"]
pub(crate) struct ActiveInterrupt {
    irq: IrqId,
}
impl ActiveInterrupt {
    pub fn id(&self) -> IrqId {
        self.irq
    }
}

/// Read IAR once. Special/spurious IDs do not create a completion obligation.
pub(crate) fn claim() -> Option<ActiveInterrupt> {
    with_controller(|controller| {
        assert!(
            controller.active.is_none(),
            "nested claim before completion"
        );
        let irq = controller.cpu.ack1();
        if irq.is_special() {
            return None;
        }
        controller.validate(irq);
        controller.active = Some(irq);
        Some(ActiveInterrupt { irq })
    })
}

/// Rebuild an [`IrqId`] from its INTID; `None` outside the architecture range.
pub(crate) fn from_intid(intid: u32) -> Option<IrqId> {
    match intid {
        0..=15 => Some(IrqId::sgi(intid)),
        16..=31 => Some(IrqId::ppi(intid - 16)),
        32..=1019 => Some(IrqId::spi(intid - 32)),
        _ => None,
    }
}

/// Priority drop only (EOIR): the claim obligation is discharged but the line
/// stays active in the GIC, which blocks re-delivery of the same INTID until
/// the driver deactivates it — the seL4 GICv3 user-IRQ protocol (docs/irq.md
/// §5). Used after signaling a bound user IRQ.
pub(crate) fn priority_drop(interrupt: ActiveInterrupt) {
    with_controller(|controller| {
        assert_eq!(controller.active, Some(interrupt.irq));
        // Order device-source writes before priority drop.
        barrier::dsb(barrier::SY);
        controller.cpu.eoi1(interrupt.irq);
        barrier::isb(barrier::SY);
        controller.active = None;
    });
}

/// Priority drop plus deactivate (EOIR + DIR): the claim is recycled at once.
/// Used for kernel-owned lines (timer) and unknown sources; the caller must
/// have cleared or masked the source first.
pub(crate) fn priority_and_deactivate(interrupt: ActiveInterrupt) {
    let irq = interrupt.irq;
    with_controller(|controller| {
        assert_eq!(controller.active, Some(irq));
        // Order device-source writes before priority drop/deactivation.
        barrier::dsb(barrier::SY);
        controller.cpu.eoi1(irq);
        barrier::isb(barrier::SY);
        controller.active = None;
        controller.cpu.dir(irq);
    });
}

/// Deactivate one line on behalf of its driver (`IRQHandler_Ack`): release the
/// active state so the line can fire again. A write for a non-active INTID is
/// ignored by the GIC, so this is safe at any point in the protocol.
pub(crate) fn deactivate(irq: IrqId) {
    with_controller(|controller| {
        controller.validate(irq);
        barrier::dsb(barrier::SY);
        controller.cpu.dir(irq);
        barrier::isb(barrier::SY);
    });
}
