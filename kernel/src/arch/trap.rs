use aarch64_cpu::registers::{ESR_EL1, FAR_EL1, Readable};
use core::{arch::global_asm, ptr::addr_of_mut};

use super::TrapFrame;

global_asm!(include_str!("trap.S"));

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

#[unsafe(no_mangle)]
extern "C" fn fatal_exception(frame: &TrapFrame, kind: u64, source: u64) -> ! {
    let esr = ESR_EL1.get();
    let far = FAR_EL1.get();
    unsafe {
        addr_of_mut!(LAST_FAULT).write_volatile(FaultRecord {
            kind,
            source,
            esr,
            far,
            frame: *frame,
        });
    }
    // source 0/1 = current EL; 2/3 = lower EL. User-fault handling is not implemented yet.
    // FAR is meaningful only for exception classes/ISS which define it.
    log::error!(
        "fatal exception: kind={} source={} ESR={:#x} FAR={:#x} PC={:#x} SPSR={:#x}",
        kind,
        source,
        esr,
        far,
        frame.elr,
        frame.spsr
    );
    if source == 2 {
        crate::root_task::stop(frame, true);
    }
    crate::utils::shutdown()
}

#[unsafe(no_mangle)]
extern "C" fn handle_user_sync(frame: &mut TrapFrame) {
    let esr = ESR_EL1.get();
    if esr >> 26 == 0x15 && esr & 0xffff == 0 {
        crate::root_task::syscall(frame);
        // ELR already points past SVC; do not advance it again.
    } else {
        fatal_exception(frame, 0, 2);
    }
}
