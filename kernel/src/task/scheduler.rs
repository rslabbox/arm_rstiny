use super::queue::{MAX_TASKS, RunQueue};
use crate::utils::single_core::SingleCore;
use crate::{
    arch::{
        TrapFrame, irq,
        user::{UserContext, UserEvent},
    },
    memory::AddressSpace,
};
use kernel_abi::*;

#[repr(C)]
pub(super) struct Task {
    pub(super) state: u64,
    pub(super) root: usize,
    pub(super) context: Option<UserContext>,
    pub(super) id: u64,
    pub(super) parent: u64,
    pub(super) space: Option<AddressSpace>,
    pub(super) deadline: u64,
    pub(super) wait_for: u64,
    pub(super) result: u64,
    pub(super) started: bool,
    pub(super) suspended_from: u64,
}
impl Task {
    pub(super) const fn empty() -> Self {
        Self {
            state: TASK_CREATED,
            root: 0,
            context: None,
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
    pub(super) fn terminal(&self) -> bool {
        matches!(self.state, TASK_EXITED | TASK_FAULTED)
    }
}
#[repr(C)]
pub(super) struct Scheduler {
    pub(super) tasks: [Task; MAX_TASKS],
    pub(super) current: Option<usize>,
    pub(super) queue: RunQueue,
    pub(super) generation: u64,
}
static SCHEDULER: SingleCore<Scheduler> = SingleCore::new(Scheduler {
    tasks: [const { Task::empty() }; MAX_TASKS],
    current: None,
    queue: RunQueue::new(),
    generation: 1,
});
pub(super) fn with_scheduler<T>(operation: impl FnOnce(&mut Scheduler) -> T) -> T {
    assert!(irq::masked());
    // The mutable borrow ends before restoring a task context or entering idle.
    operation(&mut SCHEDULER.borrow_mut())
}

impl Scheduler {
    pub(super) fn create(&mut self, parent: u64, space: AddressSpace) -> Result<usize, u64> {
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
    pub(super) fn lookup(&self, caller: usize, id: u64) -> Result<usize, u64> {
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
    pub(super) fn ready(&mut self, slot: usize) {
        assert_ne!(self.tasks[slot].state, TASK_READY);
        self.tasks[slot].state = TASK_READY;
        self.queue.push(slot);
    }
    pub(super) fn finish(&mut self, slot: usize, fault: bool, result: u64) {
        self.queue.remove(slot);
        let task = &mut self.tasks[slot];
        task.state = if fault { TASK_FAULTED } else { TASK_EXITED };
        task.result = result;
        task.root = 0;
        task.context = None;
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
                self.tasks[waiter].frame_mut().r[0] = OK;
                self.tasks[waiter].frame_mut().r[1] = result;
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
    pub(super) fn take_next(&mut self) -> Option<ActiveRun> {
        self.reap_orphans();
        self.wake_sleepers();
        let index = self.queue.pop()?;
        assert!(self.current.is_none());
        let task = &mut self.tasks[index];
        assert_eq!(task.state, TASK_READY);
        task.state = TASK_RUNNING;
        self.current = Some(index);
        Some(ActiveRun {
            id: task.id,
            root: task.root,
            context: task.context.take().expect("ready user context"),
        })
    }
    pub(super) fn complete_run(&mut self, active: ActiveRun, event: UserEvent) {
        let index = self.current.take().expect("active user task");
        assert_eq!(self.tasks[index].id, active.id);
        assert!(self.tasks[index].context.is_none());
        self.tasks[index].context = Some(active.context);
        match event {
            UserEvent::Syscall => {
                let outcome = self.syscall(index);
                match outcome {
                    super::syscall::Outcome::Complete(result) => match result {
                        Ok(value) => {
                            self.tasks[index].frame_mut().r[0] = OK;
                            if let Some(value) = value {
                                self.tasks[index].frame_mut().r[1] = value;
                            }
                        }
                        Err(code) => self.tasks[index].frame_mut().r[0] = code,
                    },
                    super::syscall::Outcome::Waiting => {}
                    super::syscall::Outcome::Exited(code) => self.finish(index, false, code),
                }
            }
            UserEvent::Interrupt => {}
            UserEvent::Fault(fault) => {
                crate::arch::trap::record_user_fault(self.tasks[index].frame(), &fault);
                self.finish(index, true, fault.esr);
            }
        }
        if self.tasks[index].state == TASK_RUNNING {
            self.ready(index);
        }
    }
    pub(super) fn root_state(&self) -> u64 {
        self.tasks[0].state
    }
}
impl Task {
    pub(super) fn frame(&self) -> &TrapFrame {
        self.context.as_ref().expect("saved context").frame()
    }
    pub(super) fn frame_mut(&mut self) -> &mut TrapFrame {
        self.context.as_mut().expect("saved context").frame_mut()
    }
}

/// Unique context detached from the scheduler for one IRQ-masked execution.
/// Its address space stays in the running slot; no task mutation happens in traps.
pub(super) struct ActiveRun {
    id: u64,
    root: usize,
    context: UserContext,
}
impl ActiveRun {
    pub(super) fn run(&mut self) -> UserEvent {
        // SAFETY: the single runtime owns this token, drops its scheduler borrow
        // before entry, and cannot release its task's space until run returns.
        unsafe { self.context.run(self.root) }
    }
}
