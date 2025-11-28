use alloc::{sync::Arc, task};
use kspin::SpinNoIrq;

use crate::{
    drivers::timer::current_nanoseconds,
    task::{
        manager::{IDLE_TASK, TaskManager},
        task_ref::TaskState,
        thread::JoinHandle,
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
static TASK_PID: AtomicUsize = AtomicUsize::new(0);

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
}

// Schedules the next task to run on the CPU.
fn task_schedule(curr_task: &TaskRef) {
    let cpu_id = crate::hal::percpu::cpu_id();

    let next_task = if let Some(task) = TASK_MANAGER.lock().pick_next_task(cpu_id) {
        task
    } else {
        IDLE_TASK.clone()
    };

    curr_task.switch_to(&next_task);
}

pub fn task_spawn(f: fn()) -> JoinHandle {
    let task = super::task_ops::task_create("task", f, false);
    let mut manager = TASK_MANAGER.lock();
    let task_ref = Arc::new(task);
    manager.put_prev_task(task_ref.clone(), false);
    JoinHandle { task: task_ref }
}

pub fn task_exit(curr_task: TaskRef) {
    info!(
        "Task Exited: id={}, name={}",
        curr_task.id(),
        curr_task.name()
    );

    assert!(!curr_task.is_idle());
    assert!(curr_task.state() == TaskState::Running);

    curr_task.set_state(TaskState::Exited);

    task_schedule(&curr_task);

    unreachable!("task exited!");
}

pub fn task_unblock(task: &TaskRef) {
    task.set_state(TaskState::Ready);
    {
        let mut manager = TASK_MANAGER.lock();
        manager.put_prev_task(task.clone(), false);
    }

    let curr_task = crate::hal::percpu::current_task();

    if curr_task.is_idle() {
        task_schedule(&curr_task);
    }
}

pub fn task_block(task: &TaskRef) {
    task.set_state(TaskState::Sleeping);
    trace!(
        "Task Blocked: id={}, name={}, state={:?}",
        task.id(),
        task.name(),
        task.state()
    );
    task_schedule(task);
}

pub fn task_sleep(duration: Duration) {
    let nanos = duration.as_nanos() as u64;
    let curr_task = crate::hal::percpu::current_task();
    let deadline_ns = current_nanoseconds() + nanos;

    assert!(curr_task.state() == TaskState::Running);
    assert!(!curr_task.is_idle());

    // Clone task reference for the timer callback
    let task_clone = curr_task.clone();

    // Calculate duration from now to deadline
    let now = current_nanoseconds();
    let duration = Duration::from_nanos(deadline_ns - now);

    // Set a timer to unblock the task
    crate::drivers::timer::set_timer(duration, move |_| {
        task_unblock(&task_clone);
    });

    task_block(&curr_task);
}

pub fn task_start() {
    task_schedule(&IDLE_TASK);
}

/// Idle loop for ROOT task when no other tasks are ready.
pub(crate) fn idle_loop() -> ! {
    loop {
        // info!("CPU {} idle, waiting for interrupt", percpu::cpu_id());
        // No task available, wait for interrupt
        aarch64_cpu::asm::wfi();
    }
}
