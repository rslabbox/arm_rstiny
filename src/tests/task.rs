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

        thread::yield_now();

        thread::sleep(Duration::from_millis(interval));
    }
    info!("[Task {id}] Completed!");
}

/// Test two tasks with periodic printing.
fn test_periodic_tasks() {
    info!("=== Test: Periodic Tasks ===");

    // Spawn task 1
    let task1 = thread::spawn(|| task1_periodic(50));

    // Spawn task 2
    let task2 = thread::spawn(|| task1_periodic(100));

    info!(
        "Waiting for tasks {} and {} to complete...",
        task1.id(),
        task2.id()
    );

    task1.join().unwrap();
    task2.join().unwrap();

    info!("All periodic tasks completed!");
}

/// Test task with return value.
fn test_task_return_value() {
    info!("=== Test: Task Return Value ===");

    // Spawn a task that returns an integer
    let handle1 = thread::spawn(|| {
        info!("[ReturnTask] Computing 21 + 21...");
        thread::sleep(Duration::from_millis(100));
        42i32
    });

    // Spawn a task that returns a string
    let handle2 = thread::spawn(|| {
        info!("[ReturnTask] Building greeting...");
        thread::sleep(Duration::from_millis(50));
        "Hello from task!"
    });

    // Spawn a task that returns a tuple
    let handle3 = thread::spawn(|| {
        info!("[ReturnTask] Computing tuple...");
        (100u64, 200u64, 300u64)
    });

    // Wait and get results
    let result1 = handle1.join().expect("Failed to join task 1");
    info!("[ReturnTask] Task 1 returned: {}", result1);
    assert!(result1 == 42, "Expected 42, got {}", result1);

    let result2 = handle2.join().expect("Failed to join task 2");
    info!("[ReturnTask] Task 2 returned: {}", result2);
    assert!(result2 == "Hello from task!", "Unexpected string result");

    let result3 = handle3.join().expect("Failed to join task 3");
    info!("[ReturnTask] Task 3 returned: ({}, {}, {})", result3.0, result3.1, result3.2);
    assert!(result3 == (100, 200, 300), "Unexpected tuple result");

    info!("=== Task Return Value Test Passed! ===");
}

/// Run all scheduler tests.
pub fn run_scheduler_tests() {
    warn!("\n=== Running Task Scheduler Tests ===");

    test_periodic_tasks();
    test_task_return_value();
}
