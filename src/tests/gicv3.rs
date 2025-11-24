use core::sync::atomic::{AtomicBool, Ordering};

use arm_gic::{
    IntId,
    gicv3::{GicCpuInterface, SgiTarget, SgiTargetGroup},
};

use crate::drivers::irq::{irqset_enable, irqset_register};

static IS_INTERRUPT: AtomicBool = AtomicBool::new(false);

pub fn gicv3_tests() {
    info!("Running GICv3 tests...");

    let sgi_intid = IntId::sgi(3);
    irqset_register(sgi_intid, |irq| {
        info!("SGI Interrupt Handler invoked for IRQ: {:?}", irq);
        IS_INTERRUPT.store(true, Ordering::Relaxed);
    });
    irqset_enable(sgi_intid, 0x80);
    if GicCpuInterface::send_sgi(
        sgi_intid,
        SgiTarget::List {
            affinity3: 0,
            affinity2: 0,
            affinity1: 0,
            target_list: 0b1,
        },
        SgiTargetGroup::CurrentGroup1,
    )
    .is_ok()
    {
        info!("SGI sent successfully to target CPU(s).");
    }

    // Wait for the interrupt to be handled
    for _ in 0..1_000_000 {
        if IS_INTERRUPT.load(Ordering::Relaxed) {
            break;
        }
    }

    if IS_INTERRUPT.load(Ordering::Relaxed) {
        info!("SGI interrupt was successfully handled.");
    } else {
        error!("SGI interrupt was not handled within the expected time.");
    }

    info!("GICv3 tests completed.");
}
