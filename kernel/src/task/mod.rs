//! Single-core user tasks driven by a returning execution boundary.
mod boot;
mod queue;
mod runtime;
mod scheduler;
mod syscall;
pub use boot::start_root;

pub fn start(space: crate::memory::AddressSpace, entry: u64, boot_info: u64) -> ! {
    scheduler::with_scheduler(|scheduler| {
        let index = scheduler.create(0, space).expect("cannot create root task");
        let task = &mut scheduler.tasks[index];
        task.context = Some(crate::arch::user::UserContext::new(
            crate::arch::TrapFrame::user(entry, 0, boot_info),
        ));
        task.started = true;
        scheduler.ready(index);
    });
    crate::arch::irq::init();
    runtime::run()
}
