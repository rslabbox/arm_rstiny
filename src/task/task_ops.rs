use alloc::{sync::Arc};
use kspin::SpinNoIrq;

use crate::{
    config::kernel::TINYENV_SMP,
    drivers::{power::system_off, timer::current_nanoseconds},
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
            info!("Idle task created for CPU {}: id={}", cpu_id, idle_task.id());
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
pub fn task_create(name: &'static str, entry: fn(), is_idle: bool) -> FifoTask<TaskInner> {
    let id = TASK_PID.fetch_add(1, Ordering::SeqCst);

    let parent_id = if !is_idle {
        crate::hal::percpu::current_task().id()
    } else {
        0usize
    };
    let task_inner = TaskInner::new(id, name, parent_id, is_idle, entry);

    info!(
        "Task Created: id={}, name={}, parent_id={}, is_idle={}",
        id, name, parent_id, is_idle
    );
    FifoTask::new(task_inner)
}

pub fn task_timer_tick() {
    // Placeholder for task scheduling logic
    check_events();
}

pub fn task_spawn(f: fn()) -> JoinHandle {
    let task = super::task_ops::task_create("task", f, false);
    let mut manager = TASK_MANAGER.lock();
    let task_ref = Arc::new(task);
    manager.put_prev_task(task_ref.clone(), false);
    ACTIVE_TASK_COUNT.fetch_add(1, Ordering::SeqCst);
    JoinHandle::new(task_ref)
}

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
        info!("All tasks have exited. System will halt.");
        system_off();
    }

    curr_task.switch_to(&get_idle_task());

    unreachable!("task exited!");
}

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

    curr_task.switch_to(&get_idle_task());
}

pub fn task_start() -> ! {
    idle_loop();

    // unreachable!("IDLE Task exited!");
}

pub fn get_idle_task() -> Arc<FifoTask<TaskInner>> {
    let cpu_id = crate::hal::percpu::cpu_id();
    IDLE_TASK[cpu_id].clone()
}

/// Idle loop for ROOT task when no other tasks are ready.
pub(crate) fn idle_loop() -> ! {
    let cpu_id = crate::hal::percpu::cpu_id();
    info!("Starting idle loop on CPU {}", cpu_id);
    loop {
        let pick_task = TASK_MANAGER.lock().pick_next_task(cpu_id);
        if let Some(task) = pick_task {
            let idle_task = get_idle_task();
            task.set_state(TaskState::Running);
            trace!("Idle Loop: Switching from idle to task id={},state={:?}", task.id(), task.state());
            idle_task.switch_to(&task);
            continue;
        }
        // No task available, wait for interrupt
        aarch64_cpu::asm::wfi();
    }
}
