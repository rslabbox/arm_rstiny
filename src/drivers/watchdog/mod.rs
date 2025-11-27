//! Watchdog driver integration.
//!
//! This module integrates the axwatchdog crate with the kernel,
//! providing lockup detection capabilities:
//!
//! - **Softlockup**: Detects when tasks cannot be scheduled (but interrupts work)
//! - **Hardlockup**: Detects when CPU is completely stuck (even interrupts don't run)
//! - **Deadlock**: Detects when locks are held too long
//!
//! # Usage
//!
//! 1. Call `init()` during kernel initialization
//! 2. Register tasks with `register_task()` that need monitoring
//! 3. Call `timer_tick()` from your timer interrupt handler
//! 4. Call `thread_tick()` from a high-priority kernel thread
//! 5. Tasks call `pet()` or use `HeartbeatTask` to indicate liveness
//!
//! # Example
//!
//! ```ignore
//! use crate::drivers::watchdog;
//!
//! // Define a monitored task
//! static MY_TASK: watchdog::HeartbeatTask = watchdog::HeartbeatTask::new("my_task");
//!
//! fn init() {
//!     // Register the task
//!     let handle = watchdog::register_task(&MY_TASK).unwrap();
//!     
//!     // In your task loop, pet the watchdog
//!     loop {
//!         MY_TASK.pet();
//!         // ... do work ...
//!     }
//! }
//! ```

// Allow unused for public API functions that may not be used immediately
#![allow(dead_code)]

use axwatchdog::{
    CpuHealth, NmiContext, Watchdog, WatchdogConfig, watchdog_nmi_handler, watchdog_thread_tick,
    watchdog_timer_handler,
};

use crate::TinyResult;
use crate::drivers::timer::generic_timer;
use crate::error::TinyError;

// Re-export task-related types for convenience
// These are public APIs that may not be used immediately
#[allow(unused_imports)]
pub use axwatchdog::{HeartbeatTask, RegistryError, TaskHandle, WatchdogTask};

/// Watchdog configuration for this platform.
const WATCHDOG_CONFIG: WatchdogConfig = WatchdogConfig::new()
    .with_hardlockup_thresh_secs(10) // 10 seconds for hardlockup
    .with_softlockup_thresh_secs(20) // 20 seconds for softlockup
    .with_num_cpus(1) // Single CPU for now
    .with_panic_on_hardlockup(true)
    .with_panic_on_softlockup(false);

/// Initialize the watchdog subsystem.
pub fn init() -> TinyResult<()> {
    info!("Initializing watchdog...");

    Watchdog::init(WATCHDOG_CONFIG).map_err(|e| {
        error!("Failed to initialize watchdog: {}", e);
        TinyError::WatchdogInitFailed("init failed")
    })?;

    info!("Watchdog initialized successfully");
    Ok(())
}

/// Start the watchdog.
pub fn start() -> TinyResult<()> {
    Watchdog::start().map_err(|e| {
        error!("Failed to start watchdog: {}", e);
        TinyError::WatchdogInitFailed("start failed")
    })?;

    info!("Watchdog started");
    Ok(())
}

/// Stop the watchdog.
pub fn stop() -> TinyResult<()> {
    Watchdog::stop().map_err(|e| {
        error!("Failed to stop watchdog: {}", e);
        TinyError::WatchdogInitFailed("stop failed")
    })?;

    info!("Watchdog stopped");
    Ok(())
}

/// Timer interrupt tick.
///
/// Call this from your timer interrupt handler.
/// This increments the hrtimer counter and checks for softlockup.
#[inline]
pub fn timer_tick(cpu_id: usize) {
    let now_ns = generic_timer::current_nanoseconds();
    watchdog_timer_handler(cpu_id, now_ns);
}

/// Watchdog thread tick.
///
/// Call this from a high-priority kernel thread periodically.
/// This updates the soft_timestamp to indicate the thread is running.
#[inline]
pub fn thread_tick(cpu_id: usize) {
    let now_ns = generic_timer::current_nanoseconds();
    watchdog_thread_tick(cpu_id, now_ns);
}

/// NMI handler entry point.
///
/// Call this from your NMI handler (PMU overflow or SDEI).
pub fn nmi_handler(cpu_id: usize, pc: usize, sp: usize, lr: usize) {
    let now_ns = generic_timer::current_nanoseconds();
    let ctx = NmiContext {
        cpu_id,
        pc,
        sp,
        lr,
        fp: 0,
        pstate: 0,
        timestamp_ns: now_ns,
    };
    watchdog_nmi_handler(cpu_id, &ctx);
}

/// Check the health of a CPU.
pub fn check_cpu_health(cpu_id: usize) -> CpuHealth {
    let now_ns = generic_timer::current_nanoseconds();
    axwatchdog::check_cpu_health(cpu_id, now_ns)
}

/// Pet the watchdog (indicate task is alive).
#[inline]
pub fn pet(cpu_id: usize) {
    axwatchdog::pet(cpu_id);
}

/// Record lock acquisition for deadlock detection.
#[inline]
pub fn lock_acquired(cpu_id: usize) {
    let now_ns = generic_timer::current_nanoseconds();
    axwatchdog::lock_acquired(cpu_id, now_ns);
}

/// Record lock release.
#[inline]
pub fn lock_released(cpu_id: usize) {
    axwatchdog::lock_released(cpu_id);
}

/// Get watchdog statistics.
pub fn stats() -> WatchdogStats {
    WatchdogStats {
        total_checks: Watchdog::total_checks(),
        total_failures: Watchdog::total_failures(),
        hardlockup_count: Watchdog::hardlockup_count(),
        softlockup_count: Watchdog::softlockup_count(),
        is_running: Watchdog::is_running(),
    }
}

/// Watchdog statistics.
#[derive(Debug, Clone, Copy)]
pub struct WatchdogStats {
    pub total_checks: u64,
    pub total_failures: u64,
    pub hardlockup_count: u64,
    pub softlockup_count: u64,
    pub is_running: bool,
}

// =============================================================================
// Task Registration API
// =============================================================================

/// Register a task for watchdog monitoring.
///
/// The task must implement the `WatchdogTask` trait and have a `'static` lifetime.
/// Returns a `TaskHandle` that can be used to unregister the task later.
///
/// # Example
///
/// ```ignore
/// use crate::drivers::watchdog::{self, HeartbeatTask, WatchdogTask};
///
/// // Create a static task
/// static MY_TASK: HeartbeatTask = HeartbeatTask::new("my_worker");
///
/// fn setup() {
///     // Register with the watchdog
///     let handle = watchdog::register_task(&MY_TASK).expect("Failed to register task");
///     
///     // Task is now being monitored
///     // In task loop:
///     loop {
///         MY_TASK.pet();
///         // ... do work ...
///     }
/// }
/// ```
pub fn register_task(task: &'static dyn WatchdogTask) -> Result<TaskHandle, RegistryError> {
    axwatchdog::register_task(task)
}

/// Unregister a previously registered task.
///
/// After unregistration, the task will no longer be monitored by the watchdog.
///
/// # Example
///
/// ```ignore
/// // Unregister when task is shutting down
/// watchdog::unregister_task(handle).expect("Failed to unregister task");
/// ```
pub fn unregister_task(handle: TaskHandle) -> Result<(), RegistryError> {
    axwatchdog::unregister_task(handle)
}

/// Check health of all registered tasks.
///
/// Returns `true` if all tasks are healthy, `false` otherwise.
/// This is safe to call from NMI context.
pub fn check_all_tasks() -> bool {
    axwatchdog::check_all_tasks()
}
