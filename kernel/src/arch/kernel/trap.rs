use aarch64_cpu::registers::{ESR_EL1, FAR_EL1, Readable};
use core::arch::global_asm;

use super::thread::TrapFrame;

use super::thread::user::{KernelReturnFrame, RawTrap};
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

#[unsafe(no_mangle)]
extern "C" fn fatal_exception(frame: &TrapFrame, kind: u64, source: u64) -> ! {
    let esr = ESR_EL1.get();
    let far = FAR_EL1.get();
    crate::api::faults::record_kernel_fault(frame, kind, source, esr, far);
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
