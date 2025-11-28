//! Task scheduler tests.

use core::time::Duration;

use crate::{hal::percpu, task::thread};

/// Task 1: Print every 500ms, 10 times
fn task1_periodic(interval: u64) {
    let id = thread::current_id();
    for i in 0..10 {
        info!(
            "[Task {id}] Iteration {}/10, CPU {}",
            i + 1,
            percpu::cpu_id()
        );

        thread::sleep(Duration::from_millis(interval));
    }
    info!("[Task {id}] Completed!");
}

/// Test two tasks with periodic printing.
fn test_periodic_tasks() {
    info!("=== Test: Periodic Tasks ===");

    // Spawn task 1
    let task1 = thread::spawn(|| task1_periodic(500));

    // Spawn task 2
    let task2 = thread::spawn(|| task1_periodic(1000));

    info!(
        "Waiting for tasks {} and {} to complete...",
        task1.id(),
        task2.id()
    );

    task1.join().unwrap();
    task2.join().unwrap();

    info!("All periodic tasks completed!");
}

/// Run all scheduler tests.
pub fn run_scheduler_tests() {
    warn!("\n=== Running Task Scheduler Tests ===");

    test_periodic_tasks();
}
