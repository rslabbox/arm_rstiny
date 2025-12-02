use alloc::sync::Arc;
use core::array;
use core::ops::Deref;
use intrusive_collections::{LinkedList, LinkedListAtomicLink, intrusive_adapter};

use super::{TaskRef, task_ops, task_ref::TaskInner};

/// A task wrapper for the [`FifoScheduler`].
///
/// It add extra states to use in [`linked_list::List`].
pub struct FifoTask<T> {
    inner: T,
    link: LinkedListAtomicLink,
}
impl<T> FifoTask<T> {
    /// Creates a new [`FifoTask`] from the inner task struct.
    pub const fn new(inner: T) -> Self {
        Self {
            inner,
            link: LinkedListAtomicLink::new(),
        }
    }
}
impl<T> Deref for FifoTask<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

intrusive_adapter!(NodeAdapter<T> = Arc<FifoTask<T>>: FifoTask<T> { link: LinkedListAtomicLink });

struct RunQueue {
    cpu_id: usize,
    run_queue: LinkedList<NodeAdapter<TaskInner>>,
}



/// Task manager that handles all task scheduling operations.
pub struct TaskManager {
    /// Scheduler for ready tasks.
    ready_queues: [RunQueue; crate::config::kernel::TINYENV_SMP],
}

impl TaskManager {
    pub fn new() -> Self {
        let ready_queues = array::from_fn(|cpu_id| RunQueue {
            cpu_id,
            run_queue: LinkedList::new(NodeAdapter::NEW),
        });
        Self { ready_queues }
    }

    pub fn pick_next_task(&mut self, cpu_id: usize) -> Option<TaskRef> {
        self.ready_queues[cpu_id].run_queue.pop_front()
    }

    pub fn put_prev_task(&mut self, task: TaskRef, _preempt: bool) {
        let cpu_id = task.id() % crate::config::kernel::TINYENV_SMP;
        info!(
            "Putting back task id={} to CPU {}'s run queue",
            task.id(),
            cpu_id
        );
        self.ready_queues[cpu_id].run_queue.push_back(task);
    }

    pub fn task_schedule(&mut self, _current: &TaskRef) -> bool {
        false
    }
}
