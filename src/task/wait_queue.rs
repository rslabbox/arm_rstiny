//! Wait queue implementation for sleeping tasks.

use alloc::collections::BinaryHeap;
use alloc::vec::Vec;
use core::cmp::Ordering;

use super::task::TaskRef;

/// A task waiting to be woken up at a specific time.
struct WaitingTask {
    /// The task reference.
    task: TaskRef,
    /// Wake time in nanoseconds (absolute time).
    wake_time_ns: u64,
}

impl PartialEq for WaitingTask {
    fn eq(&self, other: &Self) -> bool {
        self.wake_time_ns == other.wake_time_ns
    }
}

impl Eq for WaitingTask {}

impl PartialOrd for WaitingTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WaitingTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior (earliest wake time first)
        other.wake_time_ns.cmp(&self.wake_time_ns)
    }
}

/// Wait queue for tasks that are sleeping.
pub struct WaitQueue {
    /// Priority queue of waiting tasks, ordered by wake time.
    queue: BinaryHeap<WaitingTask>,
}

impl WaitQueue {
    /// Creates a new empty wait queue.
    pub const fn new() -> Self {
        Self {
            queue: BinaryHeap::new(),
        }
    }

    /// Adds a task to the wait queue with the specified wake time.
    pub fn add(&mut self, task: TaskRef, wake_time_ns: u64) {
        self.queue.push(WaitingTask { task, wake_time_ns });
    }

    /// Returns the earliest wake time in the queue, if any.
    pub fn next_wake_time(&self) -> Option<u64> {
        self.queue.peek().map(|t| t.wake_time_ns)
    }

    /// Removes and returns all tasks that should be woken up by the given time.
    pub fn wake_expired(&mut self, current_ns: u64) -> Vec<TaskRef> {
        let mut woken = Vec::new();

        while let Some(waiting) = self.queue.peek() {
            if waiting.wake_time_ns <= current_ns {
                if let Some(task) = self.queue.pop() {
                    woken.push(task.task);
                }
            } else {
                break;
            }
        }

        woken
    }

    /// Checks if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Returns the number of waiting tasks.
    pub fn len(&self) -> usize {
        self.queue.len()
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
