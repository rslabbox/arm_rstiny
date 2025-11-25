//! Task manager implementation.

use crate::{config::kernel, hal::TrapFrame};
use alloc::vec::Vec;
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    string::String,
};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::task::{Task, TaskId, TaskState};

/// Global task ID generator.
static NEXT_TASK_ID: AtomicUsize = AtomicUsize::new(1); // 0 is reserved for ROOT

/// Task manager.
pub struct TaskManager {
    /// All tasks indexed by TaskId.
    pub(super) tasks: BTreeMap<TaskId, Box<Task>>,
    /// Ready queue (tasks ready to run).
    ready_queue: VecDeque<TaskId>,
    /// Currently running task ID.
    pub(super) current_task: Option<TaskId>,
    /// ROOT task ID (idle task).
    pub(super) root_task_id: TaskId,
}

impl TaskManager {
    /// Create a new task manager with a ROOT task.
    pub fn new() -> Self {
        let mut root_task = Task::new_idle();
        let root_id = root_task.id;

        // Initialize ROOT task context
        root_task.init_context(idle_task_entry as usize, 0, task_exit as usize);

        let mut tasks = BTreeMap::new();
        tasks.insert(root_id, Box::new(root_task));

        // ROOT is not added to ready_queue (it's always available as fallback)
        let ready_queue = VecDeque::new();

        Self {
            tasks,
            ready_queue,
            current_task: None,
            root_task_id: root_id,
        }
    }

    /// Add a task to the scheduler.
    pub fn add_task(&mut self, task: Task) -> TaskId {
        let task_id = task.id;
        self.tasks.insert(task_id, Box::new(task));
        self.ready_queue.push_back(task_id);
        task_id
    }

    /// Spawn a new task with specified parent.
    ///
    /// # Arguments
    /// * `name` - Task name
    /// * `entry` - Entry point function address
    /// * `arg` - Argument passed to entry function
    /// * `parent_id` - Parent task ID (None means ROOT is parent)
    ///
    /// # Returns
    /// TaskId of the newly created task
    pub fn spawn_with_parent(
        &mut self,
        name: String,
        entry: usize,
        arg: usize,
        parent_id: Option<TaskId>,
    ) -> TaskId {
        let task_id = TaskId::new(NEXT_TASK_ID.fetch_add(1, Ordering::SeqCst));

        // If no parent specified, use ROOT
        let parent_id = parent_id.or(Some(self.root_task_id));

        let mut task = Task::new(task_id, name, parent_id);
        task.init_context(entry, arg, task_exit as usize);

        // Add to parent's children list
        if let Some(pid) = parent_id {
            if let Some(parent) = self.tasks.get_mut(&pid) {
                parent.children.push(task_id);
            }
        }

        self.add_task(task)
    }

    /// Get current running task ID.
    pub fn current_task_id(&self) -> Option<TaskId> {
        self.current_task
    }

    /// Main scheduling function.
    ///
    /// Called on timer interrupt or when a task yields/sleeps.
    pub fn schedule(&mut self, tf: &mut TrapFrame) {
        // Decrement current task's time slice
        if let Some(current_id) = self.current_task {
            if let Some(current) = self.tasks.get_mut(&current_id) {
                if current.state == TaskState::Running {
                    if current.ticks_remaining > 0 {
                        current.ticks_remaining -= 1;
                    }

                    // If time slice expired, move to back of ready queue
                    if current.ticks_remaining == 0 {
                        current.state = TaskState::Ready;
                        current.context = *tf;
                        self.ready_queue.push_back(current_id);
                    } else {
                        // Time slice not expired, continue running
                        return;
                    }
                }
            }
        }

        // Pick next task from ready queue
        if let Some(next_id) = self.ready_queue.pop_front() {
            self.switch_to(next_id, tf);
        } else {
            // No ready tasks, switch to ROOT
            self.switch_to(self.root_task_id, tf);
        }
    }

    /// Wake up a sleeping task.
    ///
    /// This is called from timer callbacks to wake up sleeping tasks.
    pub fn wake_task(&mut self, task_id: TaskId) {
        if let Some(task) = self.tasks.get_mut(&task_id) {
            if task.state == TaskState::Sleeping {
                debug!("Waking task {}", task_id.as_usize());
                task.state = TaskState::Ready;
                self.ready_queue.push_back(task_id);
            }
        }
    }

    /// Mark a task as sleeping.
    ///
    /// This removes the task from scheduling until it's woken up.
    pub fn mark_sleeping(&mut self, task_id: TaskId) {
        if let Some(task) = self.tasks.get_mut(&task_id) {
            debug!("Task {} going to sleep", task_id.as_usize());
            task.state = TaskState::Sleeping;
        }
    }

    /// Switch to a specific task.
    fn switch_to(&mut self, next_id: TaskId, tf: &mut TrapFrame) {
        if let Some(next_task) = self.tasks.get_mut(&next_id) {
            next_task.state = TaskState::Running;
            next_task.ticks_remaining = kernel::DEFAULT_TIME_SLICE;

            // Load next task's context
            *tf = next_task.context;

            self.current_task = Some(next_id);
        }
    }

    /// Exit current task.
    pub fn exit_current_task(&mut self, tf: &mut TrapFrame) {
        if let Some(current_id) = self.current_task {
            // Prevent ROOT from exiting
            if current_id == self.root_task_id {
                panic!("ROOT task cannot exit!");
            }

            info!("Task {} exiting", current_id.as_usize());

            // 1. Get children list
            let children = if let Some(task) = self.tasks.get(&current_id) {
                task.children.clone()
            } else {
                Vec::new()
            };

            // 2. Re-parent children to ROOT task
            if let Some(root) = self.tasks.get_mut(&self.root_task_id) {
                root.children.extend(children.iter());
            }

            for child_id in children {
                if let Some(child) = self.tasks.get_mut(&child_id) {
                    child.parent_id = Some(self.root_task_id);
                }
            }

            // 3. Remove from ready queue (if exists)
            self.ready_queue.retain(|&id| id != current_id);

            // 4. Delete task from tasks map (releases memory)
            self.tasks.remove(&current_id);

            // 5. Force switch to ROOT
            self.current_task = None;
            self.switch_to(self.root_task_id, tf);
        }
    }
}

/// Idle task entry point.
fn idle_task_entry() -> ! {
    loop {
        // Wait for interrupt
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

/// Task exit handler.
extern "C" fn task_exit() {
    // This will be called by the task module
    crate::task::exit_current_task();
}
