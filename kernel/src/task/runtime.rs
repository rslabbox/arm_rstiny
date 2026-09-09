//! Per-task user loop; the creator supplies the syscall policy.
use super::{
    execution::Execution,
    scheduler::{Disposition, park, with_scheduler},
};
use crate::{
    arch::kernel::thread::user::{UserContext, UserEvent},
    memory::Error,
};

pub(super) fn new_user_task(
    uctx: UserContext,
    mut dispatch_syscall: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
) -> Result<Execution, Error> {
    Execution::start(uctx, move |uctx| {
        run_user_thread_loop(uctx, &mut dispatch_syscall);
    })
}

fn run_user_thread_loop(
    uctx: &mut UserContext,
    dispatch_syscall: &mut impl FnMut(&mut UserContext) -> Disposition,
) -> ! {
    loop {
        let (root, ipc_buffer) = with_scheduler(|scheduler| scheduler.current_root());
        // SAFETY: this task owns the context; its address space stays alive while
        // running. No scheduler borrow crosses EL0 or a kernel context switch.
        let event = unsafe { uctx.run(root, ipc_buffer) };
        let action = match event {
            UserEvent::Syscall => dispatch_syscall(uctx),
            UserEvent::Interrupt => {
                if crate::interrupt::handle() == crate::interrupt::Outcome::Continue {
                    continue;
                }
                Disposition::Resume
            }
            UserEvent::Fault(fault) => crate::api::faults::handle_user_fault(uctx.frame(), &fault),
        };
        // All per-iteration locals are plain values. Captured context/handler
        // stay owned by Execution, so destruction never leaks stack resources.
        assert!(park(action).is_none());
    }
}
