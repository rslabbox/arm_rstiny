use super::{
    execution::Execution,
    queue::{MAX_TASKS, RunQueue},
    runtime::new_user_task,
};
use crate::utils::single_core::SingleCore;
use crate::{
    arch::{
        TrapFrame, irq,
        kernel_context::{self, KernelContext},
        user::UserContext,
    },
    memory::AddressSpace,
};
use kernel_abi::*;
#[path = "api.rs"]
pub(crate) mod api;

/// Scheduling decisions, independent of syscall numbers and register encoding.
pub(crate) enum Disposition {
    Resume,
    Suspend,
    Sleep(u64),
    Wait(u64),
    Exit(u64),
    Fault(u64),
}

pub(super) struct Task {
    state: u64,
    root: usize,
    execution: Option<Execution>,
    completion: Option<u64>,
    id: u64,
    parent: u64,
    space: Option<AddressSpace>,
    deadline: u64,
    wait_for: u64,
    result: u64,
    started: bool,
    suspended_from: u64,
}
impl Task {
    const fn empty() -> Self {
        Self {
            state: TASK_CREATED,
            root: 0,
            execution: None,
            completion: None,
            id: 0,
            parent: 0,
            space: None,
            deadline: 0,
            wait_for: 0,
            result: 0,
            started: false,
            suspended_from: TASK_CREATED,
        }
    }
    fn terminal(&self) -> bool {
        matches!(self.state, TASK_EXITED | TASK_FAULTED)
    }
}
pub(super) struct Scheduler {
    tasks: [Task; MAX_TASKS],
    current: Option<usize>,
    queue: RunQueue,
    generation: u64,
    switch: Option<SwitchLink>,
    pending: Option<Disposition>,
}
static SCHEDULER: SingleCore<Scheduler> = SingleCore::new(Scheduler {
    tasks: [const { Task::empty() }; MAX_TASKS],
    current: None,
    queue: RunQueue::new(),
    generation: 1,
    switch: None,
    pending: None,
});
pub(super) fn with_scheduler<T>(operation: impl FnOnce(&mut Scheduler) -> T) -> T {
    assert!(irq::masked());
    // The mutable borrow ends before restoring a task context or entering idle.
    operation(&mut SCHEDULER.borrow_mut())
}

impl Scheduler {
    fn current_slot(&self) -> Option<usize> {
        self.current
    }
    pub(super) fn current_root(&self) -> usize {
        self.tasks[self.current.expect("user execution outside task")].root
    }
    pub(super) fn current_id(&self) -> Option<u64> {
        self.current_slot().map(|slot| self.tasks[slot].id)
    }
    fn create(&mut self, parent: u64, space: AddressSpace) -> Result<usize, u64> {
        let slot = self
            .tasks
            .iter()
            .position(|task| task.id == 0)
            .ok_or(NO_MEMORY)?;
        let id = self
            .generation
            .checked_mul(MAX_TASKS as u64)
            .and_then(|g| g.checked_add(slot as u64))
            .ok_or(NO_MEMORY)?;
        self.generation = self.generation.checked_add(1).ok_or(NO_MEMORY)?;
        self.tasks[slot] = Task {
            id,
            parent,
            root: space.root(),
            space: Some(space),
            ..Task::empty()
        };
        Ok(slot)
    }
    fn lookup(&self, caller: usize, id: u64) -> Result<usize, u64> {
        let index = (id % MAX_TASKS as u64) as usize;
        let task = &self.tasks[index];
        if id == 0 || task.id != id {
            return Err(NOT_FOUND);
        }
        if index != caller && task.parent != self.tasks[caller].id {
            return Err(PERMISSION_DENIED);
        }
        Ok(index)
    }
    fn ready(&mut self, slot: usize) {
        assert_ne!(self.tasks[slot].state, TASK_READY);
        self.tasks[slot].state = TASK_READY;
        self.queue.push(slot);
    }
    fn finish(&mut self, slot: usize, fault: bool, result: u64) {
        self.queue.remove(slot);
        let task = &mut self.tasks[slot];
        task.state = if fault { TASK_FAULTED } else { TASK_EXITED };
        task.result = result;
        task.root = 0;
        task.execution = None;
        task.space = None; // We already switched back to the kernel's page table.
        let id = task.id;
        let root_id = if self.tasks[0].terminal() {
            0
        } else {
            self.tasks[0].id
        };
        for child in &mut self.tasks {
            if child.parent == id {
                child.parent = root_id;
            }
        }
        for waiter in 0..MAX_TASKS {
            let suspended_wait = self.tasks[waiter].state == TASK_SUSPENDED
                && self.tasks[waiter].suspended_from == TASK_WAITING;
            if (self.tasks[waiter].state == TASK_WAITING || suspended_wait)
                && self.tasks[waiter].wait_for == id
            {
                self.tasks[waiter].completion = Some(result);
                self.tasks[waiter].wait_for = 0;
                if suspended_wait {
                    self.tasks[waiter].suspended_from = TASK_READY;
                } else {
                    self.ready(waiter);
                }
            }
        }
    }
    fn reap_orphans(&mut self) {
        for task in &mut self.tasks[1..] {
            if task.parent == 0 && task.terminal() {
                *task = Task::empty();
            }
        }
    }
    fn wake_sleepers(&mut self) {
        let now = irq::now();
        for index in 0..MAX_TASKS {
            if self.tasks[index].state == TASK_SLEEPING && now >= self.tasks[index].deadline {
                self.ready(index);
            }
        }
    }
    fn take_next(&mut self) -> Option<ActiveTask> {
        self.reap_orphans();
        self.wake_sleepers();
        let index = self.queue.pop()?;
        assert!(self.current.is_none());
        let task = &mut self.tasks[index];
        assert_eq!(task.state, TASK_READY);
        task.state = TASK_RUNNING;
        self.current = Some(index);
        Some(ActiveTask {
            id: task.id,
            execution: task.execution.take().expect("ready task execution"),
        })
    }
    fn complete_run(&mut self, active: ActiveTask) {
        self.switch = None;
        let disposition = self.pending.take().expect("task returned without parking");
        let index = self.current.take().expect("active user task");
        assert_eq!(self.tasks[index].id, active.id);
        assert!(self.tasks[index].execution.is_none());
        self.tasks[index].execution = Some(active.execution);
        match disposition {
            Disposition::Resume => {}
            Disposition::Suspend => {
                self.tasks[index].suspended_from = TASK_RUNNING;
                self.tasks[index].state = TASK_SUSPENDED;
            }
            Disposition::Sleep(deadline) => {
                self.tasks[index].deadline = deadline;
                self.tasks[index].state = TASK_SLEEPING;
            }
            Disposition::Wait(target) => {
                self.tasks[index].wait_for = target;
                self.tasks[index].state = TASK_WAITING;
            }
            Disposition::Exit(code) => self.finish(index, false, code),
            Disposition::Fault(code) => self.finish(index, true, code),
        }
        if self.tasks[index].state == TASK_RUNNING {
            self.ready(index);
        }
    }
    pub(super) fn install_root(
        &mut self,
        space: AddressSpace,
        entry: u64,
        boot_info: u64,
        dispatch: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
    ) {
        let execution = new_user_task(
            UserContext::new(TrapFrame::user(entry, 0, boot_info)),
            dispatch,
        )
        .expect("root kernel stack");
        let index = self.create(0, space).expect("cannot create root task");
        self.tasks[index].execution = Some(execution);
        self.tasks[index].started = true;
        self.ready(index);
    }
    fn root_state(&self) -> u64 {
        self.tasks[0].state
    }
}

/// The scheduler owns the active execution until it returns to the scheduler
/// stack. A running task cannot destroy its own stack or address space.
struct ActiveTask {
    id: u64,
    execution: Execution,
}
#[derive(Clone, Copy)]
struct SwitchLink {
    task: usize,
    scheduler: usize,
}

/// Suspend at a cancellation-safe point, retaining this task's Rust call chain.
/// No shared-state guard or stack-owned resource may cross this boundary:
/// destruction can discard the continuation without unwinding its stack.
/// Returns a wait completion only when the target has terminated.
pub(crate) fn park(disposition: Disposition) -> Option<u64> {
    assert!(irq::masked());
    let (id, link) = with_scheduler(|scheduler| {
        assert!(scheduler.pending.is_none());
        scheduler.pending = Some(disposition);
        (
            scheduler.current_id().unwrap(),
            scheduler.switch.expect("park outside task"),
        )
    });
    // SAFETY: the scheduler's suspended run frame owns both contexts and stacks.
    // No shared-state borrow survives this call. Before selecting another task
    // the scheduler clears this link; a resumed task uses its newly installed link.
    unsafe {
        kernel_context::switch(
            link.task as *mut KernelContext,
            link.scheduler as *const KernelContext,
        )
    };
    with_scheduler(|scheduler| {
        assert_eq!(scheduler.current_id(), Some(id));
        let slot = scheduler.current.unwrap();
        scheduler.tasks[slot].completion.take()
    })
}

/// Schedule kernel continuations. This loop has no user trap or syscall policy.
pub(super) fn run() -> ! {
    let mut scheduler_context = KernelContext::empty();
    loop {
        let next = with_scheduler(|scheduler| scheduler.take_next());
        if let Some(mut active) = next {
            with_scheduler(|scheduler| {
                assert!(scheduler.switch.is_none());
                scheduler.switch = Some(SwitchLink {
                    task: core::ptr::addr_of_mut!(active.execution.context) as usize,
                    scheduler: core::ptr::addr_of_mut!(scheduler_context) as usize,
                });
            });
            // SAFETY: active and scheduler_context stay at stable stack addresses
            // until this call returns. The task always switches back via park.
            unsafe {
                kernel_context::switch(
                    core::ptr::addr_of_mut!(scheduler_context),
                    core::ptr::addr_of!(active.execution.context),
                )
            };
            with_scheduler(|scheduler| scheduler.complete_run(active));
        } else {
            let state = with_scheduler(|scheduler| scheduler.root_state());
            root_idle(state);
        }
    }
}

/// Debugger boundary on the scheduler stack; x0 is the root task's state.
#[inline(never)]
#[unsafe(no_mangle)]
extern "C" fn root_idle(root_state: u64) {
    core::hint::black_box(root_state);
    irq::wait_and_service();
}
