//! Per-task user loop; the creator supplies the syscall policy.
use super::{
    execution::Execution,
    scheduler::{Disposition, park, with_scheduler},
};
use crate::{
    arch::{
        irq,
        user::{UserContext, UserEvent},
    },
    memory::Error,
};

pub(super) fn new_user_task(
    mut uctx: UserContext,
    mut dispatch_syscall: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
) -> Result<Execution, Error> {
    Execution::new(move || {
        run_user_thread_loop(&mut uctx, &mut dispatch_syscall);
    })
}

fn run_user_thread_loop(
    uctx: &mut UserContext,
    dispatch_syscall: &mut impl FnMut(&mut UserContext) -> Disposition,
) -> ! {
    loop {
        let root = with_scheduler(|scheduler| scheduler.current_root());
        // SAFETY: this task owns the context; its address space stays alive while
        // running. No scheduler borrow crosses EL0 or a kernel context switch.
        let event = unsafe { uctx.run(root) };
        let action = match event {
            UserEvent::Syscall => dispatch_syscall(uctx),
            UserEvent::Interrupt => {
                if !irq::handle() {
                    continue;
                }
                Disposition::Resume
            }
            UserEvent::Fault(fault) => {
                crate::arch::trap::record_user_fault(uctx.frame(), &fault);
                Disposition::Fault(fault.esr)
            }
        };
        // All per-iteration locals are plain values. Captured context/handler
        // stay owned by Execution, so destruction never leaks stack resources.
        assert!(park(action).is_none());
    }
}
