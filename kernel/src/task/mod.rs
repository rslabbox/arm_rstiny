//! Single-core user tasks driven by a returning execution boundary.
mod execution;
mod queue;
mod runtime;
mod scheduler;
mod stack;
pub(crate) mod tick;
pub(crate) use scheduler::{Disposition, api, park};

pub fn start(
    space: crate::memory::AddressSpace,
    entry: u64,
    boot_info: u64,
    dispatch: impl FnMut(&mut crate::arch::kernel::thread::user::UserContext) -> Disposition
    + Send
    + 'static,
) -> ! {
    scheduler::with_scheduler(|scheduler| {
        scheduler.install_root(space, entry, boot_info, dispatch);
    });
    crate::arch::machine::gic::init();
    #[cfg(feature = "kernel-test")]
    crate::test::interrupt::run();
    tick::init();
    // The EL0 execution environment must be in place before the first eret.
    crate::arch::kernel::thread::user::configure_el0_domain();
    scheduler::run()
}

/// Identity of the current user task, including its trap-handling interval.
/// Boot and idle have no current task. Returns a value, never a scheduler borrow.
pub(crate) fn current_id() -> Option<u64> {
    scheduler::with_scheduler(|scheduler| scheduler.current_id())
}
