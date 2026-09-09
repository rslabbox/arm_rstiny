//! Fault diagnostics and fault-endpoint delivery.
//!
//! A user fault or unknown syscall is composed into a fault message and sent
//! through the faulting task's fault endpoint — the slot was configured by
//! `TCB_Configure` and resolves in the faulting task's own CSpace, matching
//! seL4 non-MCS `tcbFaultHandler` semantics. A supervisor reply resumes the
//! thread at its restart PC; without a usable endpoint the task terminates.
use crate::{
    arch::kernel::thread::{TrapFrame, user::UserFault},
    task::{self, Disposition, api},
};
use core::ptr::addr_of_mut;
use kernel_abi::*;

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
    source: 2,
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

/// Send one fault to the task's fault endpoint; falls back to termination.
fn deliver_or_terminate(
    frame: &TrapFrame,
    label: u64,
    length: usize,
    mrs: [u64; 4],
    terminal: u64,
) -> Disposition {
    if let Some(id) = task::current_id() {
        let slot = api::fault_endpoint(id);
        if slot != 0 && crate::api::ipc::send_fault(id, slot, label, length, mrs, frame.elr).is_ok()
        {
            return Disposition::Resume;
        }
    }
    Disposition::Fault(terminal)
}

pub(crate) fn handle_user_fault(frame: &TrapFrame, fault: &UserFault) -> Disposition {
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
        "user fault: task={:?} ESR={:#x} FAR={:?} PC={:#x}",
        crate::task::current_id(),
        fault.esr,
        fault.far,
        frame.elr
    );
    if fault.far.is_some() {
        // seL4 VMFault message: restart IP, address, instruction flag, FSR.
        let instruction = u64::from(fault.esr >> 26 == 0x20);
        deliver_or_terminate(
            frame,
            FAULT_VM,
            4,
            [frame.elr, fault.far.unwrap_or(0), instruction, fault.esr],
            fault.esr,
        )
    } else {
        // UserException: restart IP and the exception syndrome.
        deliver_or_terminate(
            frame,
            FAULT_USER_EXCEPTION,
            2,
            [frame.elr, fault.esr, 0, 0],
            fault.esr,
        )
    }
}

pub(crate) fn unknown_syscall(frame: &TrapFrame, number: u64) -> Disposition {
    // The restart PC skips the trapping `svc`: a replied unknown syscall
    // continues after it, while faults re-execute their instruction.
    let after = frame.elr + 4;
    deliver_or_terminate(
        frame,
        FAULT_UNKNOWN_SYSCALL,
        3,
        [after, frame.usp, number, 0],
        number,
    )
}
