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
    let task1 = thread::spawn(task1_periodic);

    // Spawn task 2
    let task2 = thread::spawn(task2_periodic);

    info!("Waiting for tasks {} and {} to complete...", task1.id(), task2.id());

    task1.join().unwrap();
    task2.join().unwrap();

    info!("All periodic tasks completed!");
}

/// Run all scheduler tests.
pub fn run_scheduler_tests() {
    warn!("\n=== Running Task Scheduler Tests ===");

    test_periodic_tasks();
}
