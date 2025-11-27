//! Task scheduler tests.

use core::time::Duration;

use crate::task::thread;

/// Task 1: Print every 500ms, 10 times
fn task1_periodic() {
    let id = thread::current_id();
    for i in 0..10 {
        info!("[Task {id}] Iteration {}/10", i + 1);
        if i > 5 {
            thread::yield_now();
        } else {
            thread::sleep(Duration::from_millis(50));
        }
    }
    info!("[Task 1] Completed!");
}

/// Task 2: Print every 1000ms, 10 times  
fn task2_periodic() {
    let id = thread::current_id();
    for i in 0..10 {
        info!("[Task {id}] Iteration {}/10", i + 1);
        thread::sleep(Duration::from_millis(100));
    }
    info!("[Task {id}] Completed!");
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
