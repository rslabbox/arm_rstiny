use log::info;

type TrapFrame = super::context_frame::Aarch64ContextFrame;

#[unsafe(no_mangle)]
fn handle_sync_exception(_ctx: &mut TrapFrame) -> ! {
    panic!("Synchronous exception occurred");
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
