use core::arch::global_asm;

use aarch64_cpu::registers::{ESR_EL1, VBAR_EL1};
use aarch64_cpu::registers::{Readable, Writeable};

use super::TrapFrame;

global_asm!(include_str!("trap.S"));
unsafe extern "C" {
    unsafe fn exception_vector_base();
}
pub fn init() {
    VBAR_EL1.set(exception_vector_base as usize as _);
}

#[repr(u8)]
#[derive(Debug)]
#[allow(dead_code)]
enum TrapKind {
    Synchronous = 0,
    Irq = 1,
    Fiq = 2,
    SError = 3,
}

#[repr(u8)]
#[derive(Debug)]
#[allow(dead_code)]
enum TrapSource {
    CurrentSpEl0 = 0,
    CurrentSpElx = 1,
    LowerAArch64 = 2,
    LowerAArch32 = 3,
}

#[unsafe(no_mangle)]
fn invalid_exception(tf: &mut TrapFrame, kind: TrapKind, source: TrapSource) {
    panic!(
        "Invalid exception {:?} from {:?}:\n{:#x?}",
        kind, source, tf
    );
}

#[unsafe(no_mangle)]
fn handle_sync_exception(tf: &mut TrapFrame) {
    let esr = ESR_EL1.extract();
    panic!(
        "Trap @ {:#x}: ESR = {:#x} (EC {:#08b}, ISS {:#x})\n{:#x?}",
        tf.elr,
        esr.get(),
        esr.read(ESR_EL1::EC),
        esr.read(ESR_EL1::ISS),
        tf
    );
}

#[unsafe(no_mangle)]
fn handle_irq_exception(_tf: &mut TrapFrame) {
    error!("IRQ");
}
