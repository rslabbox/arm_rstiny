//! Fault diagnostics and user fault disposition; fault endpoints are not implemented.
use crate::arch::kernel::thread::{TrapFrame, user::UserFault};
use core::ptr::addr_of_mut;

#[repr(C)]
struct FaultRecord {
    kind: u64,
    source: u64,
    esr: u64,
    far: u64,
    frame: TrapFrame,
}

// Written before formatting, so silent and failed-UART builds remain debuggable.
#[unsafe(no_mangle)]
static mut LAST_FAULT: FaultRecord = FaultRecord {
    kind: 0,
    source: 0,
    esr: 0,
    far: 0,
    frame: TrapFrame {
        r: [0; 31],
        usp: 0,
        elr: 0,
        spsr: 0,
    },
};

pub(crate) fn record_kernel_fault(frame: &TrapFrame, kind: u64, source: u64, esr: u64, far: u64) {
    // SAFETY: fatal entry runs on CPU 0 with interrupts masked.
    unsafe {
        addr_of_mut!(LAST_FAULT).write_volatile(FaultRecord {
            kind,
            source,
            esr,
            far,
            frame: *frame,
        });
    }
}

pub(crate) fn handle_user_fault(frame: &TrapFrame, fault: &UserFault) -> crate::task::Disposition {
    // SAFETY: only the IRQ-masked CPU records faults, before task reclamation.
    unsafe {
        addr_of_mut!(LAST_FAULT).write_volatile(FaultRecord {
            kind: 0,
            source: 2,
            esr: fault.esr,
            far: fault.far.unwrap_or(0),
            frame: *frame,
        });
    }
    log::error!(
        "user fault: ESR={:#x} FAR={:?} PC={:#x}",
        fault.esr,
        fault.far,
        frame.elr
    );
    crate::task::Disposition::Fault(fault.esr)
}
