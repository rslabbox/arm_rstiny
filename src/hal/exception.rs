//! Exception and interrupt handling.

use aarch64_cpu::registers::{ESR_EL1, FAR_EL1};
use aarch64_cpu::registers::{Readable, VBAR_EL1, Writeable};

use crate::hal::TrapFrame;

core::arch::global_asm!(include_str!("trap.S"));

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
fn invalid_exception(tf: &TrapFrame, kind: TrapKind, source: TrapSource) {
    error!("Invalid exception {:?} from {:?}:\n{:#x?}", kind, source, tf);
    
    // Capture backtrace from trap context
    // In AArch64: fp = x29 = r[29], lr = x30 = r[30]
    let backtrace = axbacktrace::Backtrace::capture_trap(
        tf.r[29] as usize, 
        tf.elr as usize, 
        tf.r[30] as usize
    );
    error!("\n{}", backtrace);
    
    panic!(
        "Invalid exception {:?} from {:?}",
        kind, source
    );
}

#[unsafe(no_mangle)]
fn handle_irq_exception(tf: &mut TrapFrame) {
    crate::drivers::irq::gicv3::irq_handler();
    
    // After handling interrupt, check if we need to schedule
    if crate::task::is_initialized() {
        crate::task::schedule(tf);
    }
}

fn handle_instruction_abort(tf: &TrapFrame, _iss: u64) {
    error!(
        "Instruction Abort @ {:#x}, ESR={:#x}:\n{:#x?}",
        tf.elr,
        ESR_EL1.get(),
        tf,
    );
    
    // Capture backtrace from trap context
    // In AArch64: fp = x29 = r[29], lr = x30 = r[30]
    let backtrace = axbacktrace::Backtrace::capture_trap(
        tf.r[29] as usize, 
        tf.elr as usize, 
        tf.r[30] as usize
    );
    error!("\n{}", backtrace);

    panic!("Instruction Abort encountered");
}

fn handle_data_abort(tf: &TrapFrame, _iss: u64) {
    error!(
        "Data Abort @ {:#x}, ESR={:#x}, FAR={:#x}:\n{:#x?}",
        tf.elr,
        ESR_EL1.get(),
        FAR_EL1.get(),
        tf,
    );

    // Capture backtrace from trap context
    // In AArch64: fp = x29 = r[29], lr = x30 = r[30]
    let backtrace = axbacktrace::Backtrace::capture_trap(
        tf.r[29] as usize, 
        tf.elr as usize, 
        tf.r[30] as usize
    );
    error!("\n{}", backtrace);

    panic!("Data Abort encountered");
}

#[unsafe(no_mangle)]
fn handle_sync_exception(tf: &mut TrapFrame) {
    let esr = ESR_EL1.extract();
    let iss = esr.read(ESR_EL1::ISS);
    match esr.read_as_enum(ESR_EL1::EC) {
        Some(ESR_EL1::EC::Value::InstrAbortCurrentEL) => handle_instruction_abort(tf, iss),
        Some(ESR_EL1::EC::Value::DataAbortCurrentEL) => handle_data_abort(tf, iss),
        Some(ESR_EL1::EC::Value::Brk64) => {
            debug!("BRK #{:#x} @ {:#x} ", iss, tf.elr);
            tf.elr += 4;
        }
        _ => {
            panic!(
                "Unhandled synchronous exception @ {:#x}: ESR={:#x} (EC {:#08b}, ISS {:#x})",
                tf.elr,
                esr.get(),
                esr.read(ESR_EL1::EC),
                esr.read(ESR_EL1::ISS),
            );
        }
    }
}

/// Initializes trap handling on the current CPU.
///
/// In detail, it initializes the exception vector, and sets `TTBR0_EL1` to 0 to
/// block low address access.
pub fn init_exception() {
    unsafe extern "C" {
        fn exception_vector_base();
    }

    VBAR_EL1.set(exception_vector_base as _);
}
