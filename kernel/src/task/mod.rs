//! Single-core user tasks driven by a returning execution boundary.
mod execution;
mod queue;
mod runtime;
mod scheduler;
mod stack;
pub(crate) use scheduler::{Disposition, api, park};

pub fn start(
    space: crate::memory::AddressSpace,
    entry: u64,
    boot_info: u64,
    dispatch: impl FnMut(&mut crate::arch::user::UserContext) -> Disposition + Send + 'static,
) -> ! {
    scheduler::with_scheduler(|scheduler| {
        scheduler.install_root(space, entry, boot_info, dispatch);
    });
    crate::arch::irq::init();
    scheduler::run()
}

/// Identity of the current user task, including its trap-handling interval.
/// Boot and idle have no current task. Returns a value, never a scheduler borrow.
pub(crate) fn current_id() -> Option<u64> {
    scheduler::with_scheduler(|scheduler| scheduler.current_id())
}
