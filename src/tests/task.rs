//! Task scheduler tests.

use core::time::Duration;

use crate::task::thread;

/// Task 1: Print every 500ms, 10 times
fn task1_periodic() {
    for i in 0..10 {
        info!("[Task 1] Iteration {}/10", i + 1);
        thread::sleep(Duration::from_millis(500));
    }
    info!("[Task 1] Completed!");
}

/// Task 2: Print every 1000ms, 10 times  
fn task2_periodic() {
    for i in 0..10 {
        info!("[Task 2] Iteration {}/10", i + 1);
        thread::sleep(Duration::from_millis(1000));
    }
    info!("[Task 2] Completed!");
}

/// Test two tasks with periodic printing.
fn test_periodic_tasks() {
    info!("=== Test: Periodic Tasks ===");

    // Spawn task 1
    thread::spawn(task1_periodic);

    // Spawn task 2
    thread::spawn(task2_periodic);

    info!("Tasks spawned, scheduler will manage them");
}

/// Run all scheduler tests.
pub fn run_scheduler_tests() {
    info!("\n=== Running Task Scheduler Tests ===\n");

    test_periodic_tasks();

    info!("\n=== Task Scheduler Tests Complete ===\n");
    info!("Main thread continues after all tasks completed");
}
