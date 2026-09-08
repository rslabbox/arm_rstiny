use aarch64_cpu::registers::{ESR_EL1, FAR_EL1, Readable};
use core::{arch::global_asm, ptr::addr_of_mut};

use super::TrapFrame;

use super::user::{KernelReturnFrame, RawTrap, UserFault};
use core::mem::{offset_of, size_of};
global_asm!(include_str!("trap.S"),
    frame_size = const size_of::<TrapFrame>(),
    frame_pairs = const size_of::<TrapFrame>() / 16,
    return_size = const size_of::<KernelReturnFrame>(),
    context_pointer = const size_of::<TrapFrame>() + offset_of!(KernelReturnFrame, context),
    raw_pointer = const size_of::<TrapFrame>() + offset_of!(KernelReturnFrame, trap),
    kind_offset = const offset_of!(RawTrap, kind),
    esr_offset = const offset_of!(RawTrap, esr),
);

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
    // Synchronous EL0 faults return through run(); this path is kernel-fatal.
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
    panic!(
        "fatal exception: kind={kind} source={source} ESR={esr:#x} FAR={far:#x} PC={:#x}",
        frame.elr
    )
}

pub(crate) fn record_user_fault(frame: &TrapFrame, fault: &UserFault) {
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
}
