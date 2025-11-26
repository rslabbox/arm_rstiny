//! Per-CPU data structure and operations.
//!
//! This module provides per-CPU local storage using the TPIDR_EL1 register.
//! Each CPU has its own PerCpu structure that stores CPU-local data such as
//! the currently running task.

use alloc::sync::Arc;

use super::cpu::{set_thread_pointer, thread_pointer};

use crate::task::task::{SchedulableTask, TaskRef};

/// Per-CPU data structure.
///
/// This structure is stored in each CPU's TPIDR_EL1 register and contains
/// CPU-local data that can be accessed without locking.
#[repr(C)]
pub struct PerCpu {
    /// Pointer to the currently running task.
    current_task: *const SchedulableTask,
    /// Pointer to the idle task for this CPU.
    idle_task: *const SchedulableTask,
    /// The CPU ID.
    cpu_id: usize,
}

// Safety: PerCpu is only accessed by the CPU it belongs to.
unsafe impl Send for PerCpu {}
unsafe impl Sync for PerCpu {}

/// Static storage for the per-CPU area (single CPU for now).
static mut PERCPU_AREA: PerCpu = PerCpu {
    current_task: core::ptr::null(),
    idle_task: core::ptr::null(),
    cpu_id: 0,
};

impl PerCpu {
    /// Returns the current task pointer.
    #[inline]
    pub fn current_task_ptr(&self) -> *const SchedulableTask {
        self.current_task
    }

    /// Sets the current task pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` points to a valid SchedulableTask
    /// that will remain valid for the duration it is set as current.
    #[inline]
    pub unsafe fn set_current_task_ptr(&mut self, ptr: *const SchedulableTask) {
        self.current_task = ptr;
    }
}

/// Initializes the per-CPU data for the current CPU.
///
/// This function must be called early in the boot process, before any
/// task-related operations.
///
/// # Safety
///
/// This function must only be called once per CPU during initialization.
pub unsafe fn init(cpu_id: usize) {
    let percpu = &raw mut PERCPU_AREA;
    unsafe {
        (*percpu).cpu_id = cpu_id;
        (*percpu).current_task = core::ptr::null();

        // Set TPIDR_EL1 to point to the PerCpu structure
        set_thread_pointer(percpu as usize);
    }
}

/// Returns a reference to the current CPU's PerCpu structure.
///
/// # Panics
///
/// Panics if called before `init()` has been called.
#[inline]
pub fn current_cpu() -> &'static PerCpu {
    let ptr = thread_pointer() as *const PerCpu;
    assert!(!ptr.is_null(), "PerCpu not initialized");
    unsafe { &*ptr }
}

/// Returns a mutable reference to the current CPU's PerCpu structure.
///
/// # Safety
///
/// The caller must ensure exclusive access to the PerCpu structure.
#[inline]
unsafe fn current_cpu_mut() -> &'static mut PerCpu {
    let ptr = thread_pointer() as *mut PerCpu;
    assert!(!ptr.is_null(), "PerCpu not initialized");
    unsafe { &mut *ptr }
}

/// Returns the current task's raw pointer.
///
/// This is a fast path that doesn't increment the reference count.
#[inline]
pub fn current_task_ptr() -> *const SchedulableTask {
    current_cpu().current_task_ptr()
}

/// Sets the current task pointer.
///
/// # Safety
///
/// The caller must ensure:
/// - The pointer points to a valid SchedulableTask
/// - The task's reference count has been properly managed
/// - This is called during context switch or initialization
#[inline]
pub unsafe fn set_current_task_ptr(ptr: *const SchedulableTask) {
    unsafe {
        current_cpu_mut().set_current_task_ptr(ptr);
    }
}

/// Returns a reference to the current task (increments reference count).
///
/// # Panics
///
/// Panics if no current task is set.
#[inline]
pub fn current_task() -> TaskRef {
    let ptr = current_task_ptr();
    assert!(!ptr.is_null(), "No current task set");

    // Increment reference count by creating Arc from raw and cloning
    let arc = unsafe { Arc::from_raw(ptr) };
    let cloned = arc.clone();
    // Don't decrement the original reference count
    core::mem::forget(arc);
    cloned
}

/// Sets the current task from a TaskRef.
///
/// This properly manages the reference count:
/// - Increments the ref count for the new task
/// - The old task's ref count remains unchanged (caller's responsibility)
#[inline]
pub fn set_current_task(task: &TaskRef) {
    let ptr = Arc::as_ptr(task);
    // Increment reference count for the new current task
    core::mem::forget(task.clone());
    unsafe {
        set_current_task_ptr(ptr);
    }
}

/// Sets the idle task for this CPU.
///
/// This should be called once during initialization after the idle task is created.
#[inline]
pub fn set_idle_task(task: &TaskRef) {
    let ptr = Arc::as_ptr(task);
    // Increment reference count for the idle task
    core::mem::forget(task.clone());
    unsafe {
        current_cpu_mut().idle_task = ptr;
    }
}

/// Returns a reference to the idle task (increments reference count).
///
/// # Panics
///
/// Panics if no idle task is set.
#[inline]
pub fn idle_task() -> TaskRef {
    let ptr = unsafe { current_cpu_mut().idle_task };
    assert!(!ptr.is_null(), "No idle task set");

    // Increment reference count by creating Arc from raw and cloning
    let arc = unsafe { Arc::from_raw(ptr) };
    let cloned = arc.clone();
    // Don't decrement the original reference count
    core::mem::forget(arc);
    cloned
}
