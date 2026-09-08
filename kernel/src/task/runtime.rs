//! Own the kernel call chain; traps return events and never select tasks.
use super::scheduler::with_scheduler;
use crate::arch::{irq, user::UserEvent};

pub(super) fn run() -> ! {
    loop {
        let next = with_scheduler(|scheduler| scheduler.take_next());
        match next {
            Some(mut active) => {
                let mut event = active.run();
                // An unrelated/spurious IRQ is not a consumed time slice.
                while matches!(event, UserEvent::Interrupt) && !irq::handle() {
                    event = active.run();
                }
                with_scheduler(|scheduler| scheduler.complete_run(active, event));
            }
            None => {
                let state = with_scheduler(|scheduler| scheduler.root_state());
                root_idle(state);
            }
        }
    }
}

/// Stable debugger boundary: x0 describes the root task, independent of its layout.
/// WFI wakes for a pending IRQ even with DAIF.I set; service it before selecting.
#[inline(never)]
#[unsafe(no_mangle)]
extern "C" fn root_idle(root_state: u64) {
    core::hint::black_box(root_state);
    irq::wait_and_service();
}
