use alloc::{sync::Arc};
use kspin::SpinNoIrq;

use crate::{
    config::kernel::TINYENV_SMP,
    drivers::{power::system_off, timer::{busy_wait, current_nanoseconds}},
    task::{
        manager::TaskManager,
        task_ref::TaskState,
        thread::JoinHandle,
        timers::{check_events, set_timer},
    },
};

use super::TaskRef;
use super::manager::FifoTask;
use core::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use super::task_ref::TaskInner;

/// PID of the tasks
static TASK_PID: AtomicUsize = AtomicUsize::new(1);

static ACTIVE_TASK_COUNT: AtomicUsize = AtomicUsize::new(0);

lazy_static::lazy_static! {
    /// Public lock for accessing the task manager.
    pub static ref IDLE_TASK: [Arc<FifoTask<TaskInner>>; TINYENV_SMP] = {
        core::array::from_fn(|cpu_id| {
            // Placeholder idle task for initialization
            let idle_task = task_create("idle", || idle_loop(), true);
            // Idle is aways running
            idle_task.set_state(TaskState::Running);
            debug!("Idle task created for CPU {}: id={}", cpu_id, idle_task.id());
            Arc::new(idle_task)
        })
    };
}

lazy_static::lazy_static! {
    pub static ref TASK_MANAGER: SpinNoIrq<TaskManager> = {
        SpinNoIrq::new(TaskManager::new())
    };
}

/// Creates a new task and returns its TaskRef.
pub fn task_create<F, T>(name: &'static str, entry: F, is_idle: bool) -> FifoTask<TaskInner>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let id = TASK_PID.fetch_add(1, Ordering::SeqCst);

    let parent_id = if !is_idle {
        crate::hal::percpu::current_task().id()
    } else {
        0usize
    };
    let task_inner = TaskInner::new(id, name, parent_id, is_idle, entry);

    debug!(
        "Task Created: id={}, name={}, parent_id={}, is_idle={}",
        id, name, parent_id, is_idle
    );
    FifoTask::new(task_inner)
}

/// Timer tick handler for task scheduling.
/// Checks and processes expired timer events to wake up sleeping tasks.
pub fn task_timer_tick() {
    check_events();
}

/// Spawns a new user task with the given entry function.
/// Adds the task to the ready queue and returns a JoinHandle for synchronization.
pub fn task_spawn<F, T>(name: &'static str,f: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let task = super::task_ops::task_create(name, f, false);
    let mut manager = TASK_MANAGER.lock();
    let task_ref = Arc::new(task);
    manager.put_prev_task(task_ref.clone(), false);
    ACTIVE_TASK_COUNT.fetch_add(1, Ordering::SeqCst);
    JoinHandle::new(task_ref)
}

/// Switches the current task back to the idle task.
/// Called when a task yields, sleeps, or exits.
fn task_drop_to_idle(curr_task: &TaskRef) {
    let idle_task = get_idle_task();
    curr_task.switch_to(&idle_task);
}

/// Voluntarily yields the CPU to other tasks.
/// Puts the current task back into the ready queue and switches to idle.
pub fn task_yield() {
    let curr_task = crate::hal::percpu::current_task();
    let cpu_id = crate::hal::percpu::cpu_id();

    assert!(curr_task.state() == TaskState::Running);
    assert!(!curr_task.is_idle());

    debug!(
        "Task Yielding: id={}, name={}, cpu={}",
        curr_task.id(),
        curr_task.name(),
        cpu_id
    );

    curr_task.set_state(TaskState::Ready);
    TASK_MANAGER
        .lock()
        .put_prev_task(curr_task.clone(), true);

    task_drop_to_idle(&curr_task);
}

/// Handles task exit and cleanup.
/// Sets the task state to Exited and switches to idle. Shuts down the system if no tasks remain.
pub fn task_exit(curr_task: TaskRef) {
    let cpu_id = crate::hal::percpu::cpu_id();
    debug!(
        "Task Exited: id={}, name={}, cpu={}",
        curr_task.id(),
        curr_task.name(),
        cpu_id
    );

    assert!(!curr_task.is_idle());
    assert!(curr_task.state() == TaskState::Running);

    curr_task.set_state(TaskState::Exited);

    let remaining = ACTIVE_TASK_COUNT.fetch_sub(1, Ordering::SeqCst) - 1;
    if remaining == 0 {
        debug!("All tasks have exited. System will halt.");
        system_off();
    }

    task_drop_to_idle(&curr_task);

    unreachable!("task exited!");
}

/// Puts the current task to sleep for the specified duration.
/// Sets a timer and switches to idle until the deadline is reached.
pub fn task_sleep(duration: Duration) {
    let nanos = duration.as_nanos() as u64;
    let curr_task = crate::hal::percpu::current_task();
    let deadline_ns = current_nanoseconds() + nanos;

    assert!(curr_task.state() == TaskState::Running);
    assert!(!curr_task.is_idle());

    // Calculate duration from now to deadline
    let now = current_nanoseconds();
    let duration = Duration::from_nanos(deadline_ns - now);

    debug!(
        "Task Sleeping: id={}, name={}, duration={:?}",
        curr_task.id(),
        curr_task.name(),
        duration
    );

    curr_task.set_state(TaskState::Sleeping);
    set_timer(deadline_ns, &curr_task);

    task_drop_to_idle(&curr_task);
}

/// Starts the task scheduling system.
/// Enters the idle loop and never returns.
pub fn task_start() -> ! {
    idle_loop();

    // unreachable!("IDLE Task exited!");
}

/// Returns the idle task reference for the current CPU.
pub fn get_idle_task() -> Arc<FifoTask<TaskInner>> {
    let cpu_id = crate::hal::percpu::cpu_id();
    IDLE_TASK[cpu_id].clone()
}

/// Idle loop for ROOT task when no other tasks are ready.
pub(crate) fn idle_loop() -> ! {
    let cpu_id = crate::hal::percpu::cpu_id();
    debug!("Starting idle loop on CPU {}", cpu_id);
    loop {
        let pick_task = TASK_MANAGER.lock().pick_next_task(cpu_id);
        if let Some(task) = pick_task {
            let idle_task = get_idle_task();
            task.set_state(TaskState::Running);
            trace!("Idle Loop: Switching from idle to task id={},state={:?}", task.id(), task.state());
            // busy_wait(Duration::from_nanos(10));
            idle_task.switch_to(&task);

            continue;
        }
        // No task available, wait for interrupt
        aarch64_cpu::asm::wfi();
    }
}
