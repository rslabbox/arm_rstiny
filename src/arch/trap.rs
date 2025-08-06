use log::info;
use core::arch::asm;

type TrapFrame = super::context_frame::Aarch64ContextFrame;

#[unsafe(no_mangle)]
fn handle_sync_exception(ctx: &mut TrapFrame) -> ! {
    // Read ESR_EL1 to get exception information
    let esr: u64;
    unsafe {
        asm!("mrs {}, esr_el1", out(reg) esr);
    }

    let exception_class = (esr >> 26) & 0x3F;
    let instruction_specific = esr & 0x1FFFFFF;

    panic!(
        "Synchronous exception occurred:\n\
         ESR_EL1: 0x{:016x}\n\
         Exception Class: 0x{:02x}\n\
         ISS: 0x{:07x}\n\
         ELR_EL1: 0x{:016x}\n\
         FAR_EL1: 0x{:016x}\n\
         Context: {:#x?}",
        esr,
        exception_class,
        instruction_specific,
        ctx.elr,
        read_far_el1(),
        ctx
    );
}

fn read_far_el1() -> u64 {
    let far: u64;
    unsafe {
        asm!("mrs {}, far_el1", out(reg) far);
    }
    far
}

#[unsafe(no_mangle)]
fn invalid_exception(tf: &TrapFrame, kind: usize, source: usize) {
    panic!(
        "Invalid exception {:?} from {:?}:\n{:#x?}",
        kind, source, tf
    );
}

#[unsafe(no_mangle)]
fn handle_irq_exception(_tf: &TrapFrame) {
    info!("IRQ trap");
}
