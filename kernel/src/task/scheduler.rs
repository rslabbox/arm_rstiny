use super::{
    execution::Execution,
    queue::{MAX_TASKS, RunQueue},
    runtime::new_user_task,
};
use crate::utils::single_core::SingleCore;
use crate::{
    arch::{
        kernel::thread::{
            TrapFrame,
            kernel_context::{self, KernelContext},
            user::UserContext,
        },
        machine::{instructions, time},
    },
    object::ObjectId,
};
use kernel_abi::*;
#[path = "api.rs"]
pub(crate) mod api;

/// Scheduling decisions, independent of syscall numbers and register encoding.
/// [`Disposition::Block`] leaves the state the syscall committed: the task
/// waits on an endpoint, a reply or a fault handler and is woken by delivery.
pub(crate) enum Disposition {
    Resume,
    Sleep(u64),
    Wait(u64),
    Exit(u64),
    Fault(u64),
    Block,
}

/// A pending reply relationship. A receiver holds at most one; `Reply`
/// consumes it. Fault senders are resumed at their restart PC instead of
/// receiving a reply message.
#[derive(Clone, Copy)]
pub(crate) enum Caller {
    Call(u64),
    Fault(u64),
}
impl Caller {
    pub(crate) fn task(self) -> u64 {
        match self {
            Caller::Call(id) | Caller::Fault(id) => id,
        }
    }
}

/// Why a task is parked inside an IPC syscall. Queued senders keep their
/// message in their own saved context until delivery; fault senders carry it
/// in `fault_msg`.
#[derive(Clone, Copy)]
pub(crate) struct Blocked {
    pub ep: crate::object::ObjectId,
    pub badge: u64,
    /// The sender expects a reply: delivery grants the receiver this right.
    pub call: bool,
    pub grant_reply: bool,
    /// A faulted thread delivering through its fault endpoint.
    pub fault: bool,
}

/// A fault message held for a queued fault sender.
#[derive(Clone, Copy)]
pub(crate) struct FaultMsg {
    pub label: u64,
    pub length: usize,
    pub mrs: [u64; 4],
}

pub(super) struct Task {
    state: u64,
    root: usize,
    cspace: Option<ObjectId>,
    vspace: Option<ObjectId>,
    ipc_buffer: usize,
    execution: Option<Execution>,
    completion: Option<u64>,
    id: u64,
    parent: u64,
    deadline: u64,
    wait_for: u64,
    result: u64,
    started: bool,
    suspended_from: u64,
    fault_ep: u64,
    blocked: Option<Blocked>,
    caller: Option<Caller>,
    restart_pc: u64,
    fault_msg: Option<FaultMsg>,
}
impl Task {
    const fn empty() -> Self {
        Self {
            state: TASK_CREATED,
            root: 0,
            cspace: None,
            vspace: None,
            ipc_buffer: 0,
            execution: None,
            completion: None,
            id: 0,
            parent: 0,
            deadline: 0,
            wait_for: 0,
            result: 0,
            started: false,
            suspended_from: TASK_CREATED,
            fault_ep: 0,
            blocked: None,
            caller: None,
            restart_pc: 0,
            fault_msg: None,
        }
    }
    fn terminal(&self) -> bool {
        matches!(self.state, TASK_EXITED | TASK_FAULTED)
    }
    /// Blocked on a wait queue: an endpoint or notification holds this task.
    fn queued(&self) -> bool {
        matches!(self.state, TASK_BLOCKED_SEND | TASK_BLOCKED_RECV)
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
    assert!(instructions::irq_masked());
    // The mutable borrow ends before restoring a task context or entering idle.
    operation(&mut SCHEDULER.borrow_mut())
}

impl Scheduler {
    fn current_slot(&self) -> Option<usize> {
        self.current
    }
    pub(super) fn current_root(&self) -> (usize, usize) {
        let task = &self.tasks[self.current.expect("user execution outside task")];
        (task.root, task.ipc_buffer)
    }
    pub(super) fn current_id(&self) -> Option<u64> {
        self.current_slot().map(|slot| self.tasks[slot].id)
    }
    fn create(&mut self, parent: u64) -> Result<usize, u64> {
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
            ..Task::empty()
        };
        Ok(slot)
    }
    fn lookup(&self, id: u64) -> Result<usize, u64> {
        let index = (id % MAX_TASKS as u64) as usize;
        let task = &self.tasks[index];
        if id == 0 || task.id != id {
            return Err(NOT_FOUND);
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
        task.vspace = None; // We already switched back to the kernel's page table.
        let id = task.id;
        // A partner blocked on this task's reply (or a faulted thread waiting
        // for its supervisor, whose supervisor is now gone) must not hang:
        // completion `Some(0)` makes the blocked continuation fail its call.
        let waiting = task.caller.take().map(Caller::task);
        task.blocked = None;
        crate::object::retire_task(id);
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
        if let Some(waiter) = waiting {
            if let Ok(target) = self.lookup(waiter) {
                let task = &self.tasks[target];
                let waiting_for_reply =
                    matches!(task.state, TASK_BLOCKED_REPLY | TASK_BLOCKED_FAULT)
                        || (task.state == TASK_SUSPENDED
                            && matches!(
                                task.suspended_from,
                                TASK_BLOCKED_REPLY | TASK_BLOCKED_FAULT
                            ));
                if waiting_for_reply {
                    self.tasks[target].completion = Some(0);
                    self.ready(target);
                }
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
        let now = time::now();
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
            // A yielding task rejoins the queue. A task that suspended or
            // blocked itself during the syscall keeps its committed state; a
            // `Block` task is re-queued only by its waker.
            Disposition::Resume if self.tasks[index].state == TASK_RUNNING => self.ready(index),
            Disposition::Resume => {}
            Disposition::Block => {}
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
    }
    pub(super) fn install_root(
        &mut self,
        vspace: ObjectId,
        root: usize,
        entry: u64,
        boot_info: u64,
        untyped_start: u64,
        dispatch: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
    ) {
        let execution = new_user_task(
            UserContext::new(TrapFrame::user(entry, 0, boot_info)),
            dispatch,
        )
        .expect("root kernel stack");
        let index = self.create(0).expect("cannot create root task");
        self.tasks[index].execution = Some(execution);
        self.tasks[index].ipc_buffer = boot_info as usize - crate::memory::PAGE_SIZE;
        self.tasks[index].root = root;
        self.tasks[index].vspace = Some(vspace);
        self.tasks[index].cspace = Some(crate::object::init_root(
            self.tasks[index].id,
            vspace,
            self.tasks[index].ipc_buffer,
            untyped_start,
        ));
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
    assert!(instructions::irq_masked());
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
    let mut scheduler_context = KernelContext::default();
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
            // Task exit and destruction release objects; sweep after the
            // scheduler borrow is released so collection can inspect tasks.
            crate::object::collect_if_requested();
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
    instructions::wait_for_interrupt();
    let _ = crate::interrupt::handle();
}
