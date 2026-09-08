//! Single-core tasks. Kernel execution is non-preemptible; EL0 is time sliced.
mod boot;
mod queue;
mod syscall;
use crate::{
    arch::{TrapFrame, irq},
    memory::{self, AddressSpace},
};
pub use boot::start_root;
use core::cell::UnsafeCell;
use kernel_abi::*;
use queue::{MAX_TASKS, RunQueue};

#[repr(C)]
struct Task {
    state: u64,
    root: usize,
    context: TrapFrame,
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
            context: TrapFrame {
                r: [0; 31],
                usp: 0,
                elr: 0,
                spsr: 0,
            },
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
#[repr(C)]
struct Scheduler {
    tasks: [Task; MAX_TASKS],
    current: Option<usize>,
    queue: RunQueue,
    generation: u64,
}
#[repr(transparent)]
struct SingleCore(UnsafeCell<Scheduler>);
// SAFETY: only CPU 0 runs; all accesses are scoped to IRQ-masked EL1 handlers.
unsafe impl Sync for SingleCore {}
#[unsafe(no_mangle)]
static SCHEDULER: SingleCore = SingleCore(UnsafeCell::new(Scheduler {
    tasks: [const { Task::empty() }; MAX_TASKS],
    current: None,
    queue: RunQueue::new(),
    generation: 1,
}));
fn with_scheduler<T>(operation: impl FnOnce(&mut Scheduler) -> T) -> T {
    assert!(irq::masked());
    // SAFETY: IRQ-masked, nonrecursive, single-core access. References cannot
    // escape the closure, and no closure switches task or enables interrupts.
    operation(unsafe { &mut *SCHEDULER.0.get() })
}

impl Scheduler {
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
                self.tasks[waiter].context.r[0] = OK;
                self.tasks[waiter].context.r[1] = result;
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
    fn select(&mut self) -> Option<(usize, TrapFrame)> {
        self.reap_orphans();
        self.wake_sleepers();
        let index = self.queue.pop()?;
        assert_eq!(self.tasks[index].state, TASK_READY);
        self.tasks[index].state = TASK_RUNNING;
        self.current = Some(index);
        Some((self.tasks[index].root, self.tasks[index].context))
    }
}

pub enum Event {
    Syscall,
    Timer,
    Fault(u64),
}

/// Save the outgoing task before making any object or address-space changes.
pub fn dispatch(frame: &TrapFrame, event: Event) -> ! {
    memory::activate_kernel();
    let next = with_scheduler(|scheduler| {
        if let Some(index) = scheduler.current.take() {
            scheduler.tasks[index].context = *frame;
            match event {
                Event::Syscall => scheduler.syscall(index),
                Event::Timer => {}
                Event::Fault(esr) => scheduler.finish(index, true, esr),
            }
            if scheduler.tasks[index].state == TASK_RUNNING {
                scheduler.ready(index);
            }
        }
        scheduler.select()
    });
    enter(next)
}

fn enter(next: Option<(usize, TrapFrame)>) -> ! {
    if let Some((root, frame)) = next {
        memory::activate(root);
        // SAFETY: scheduler owns and validates all saved/initial task contexts;
        // no Rust references into scheduler state survive across eret.
        unsafe { crate::arch::user::enter(&frame) }
    } else {
        root_idle()
    }
}

pub fn start(space: AddressSpace, entry: u64) -> ! {
    let next = with_scheduler(|scheduler| {
        let index = scheduler.create(0, space).expect("cannot create root task");
        let task = &mut scheduler.tasks[index];
        task.context = TrapFrame::user(entry, 0, BOOTINFO_VA);
        task.started = true;
        scheduler.ready(index);
        scheduler.select()
    });
    irq::init();
    enter(next)
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
extern "C" fn root_idle() -> ! {
    // No suspended Rust stack frames: an idle IRQ may select any ready task.
    core::arch::naked_asm!(
        "ldr x9, =boot_stack_top",
        "mov sp, x9",
        "msr daifclr, #2",
        "2:",
        "wfi",
        "b 2b",
    );
}
