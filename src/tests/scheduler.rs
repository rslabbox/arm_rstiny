//! Task scheduler tests.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use crate::drivers::timer::busy_wait;
use crate::task::thread;

/// Test two tasks with periodic printing.
fn test_periodic_tasks() {
    info!("=== Test: Periodic Tasks ===");

    let counter1 = Arc::new(AtomicUsize::new(0));
    let counter2 = Arc::new(AtomicUsize::new(0));

    // Task 1: Print every 500ms, 10 times
    let c1 = counter1.clone();
    thread::spawn(move || {
        for i in 0..10 {
            c1.fetch_add(1, Ordering::SeqCst);
            info!("[Task 1] Iteration {}/10", i + 1);
            thread::sleep(Duration::from_millis(500));
        }
        info!("[Task 1] Completed!");
    });

    // Task 2: Print every 700ms, 10 times
    let c2 = counter2.clone();
    thread::spawn(move || {
        for i in 0..10 {
            c2.fetch_add(1, Ordering::SeqCst);
            info!("[Task 2] Iteration {}/10", i + 1);
            thread::sleep(Duration::from_millis(1000));
        }
        info!("[Task 2] Completed!");
    });

    info!("Waiting for tasks to complete...");

    // Poll until both tasks complete
    loop {
        let count1 = counter1.load(Ordering::SeqCst);
        let count2 = counter2.load(Ordering::SeqCst);

        if count1 >= 10 && count2 >= 10 {
            break;
        }

        busy_wait(Duration::from_millis(100));
    }

    let count1 = counter1.load(Ordering::SeqCst);
    let count2 = counter2.load(Ordering::SeqCst);

    info!("Task 1 iterations: {}", count1);
    info!("Task 2 iterations: {}", count2);

    if count1 == 10 && count2 == 10 {
        info!("✓ Periodic tasks test PASSED");
    } else {
        error!(
            "✗ Periodic tasks test FAILED: task1={}, task2={}",
            count1, count2
        );
    }
}

/// Run all scheduler tests.
pub fn run_scheduler_tests() {
    info!("\n=== Running Task Scheduler Tests ===\n");

    test_periodic_tasks();

    info!("\n=== Task Scheduler Tests Complete ===\n");
    info!("Main thread continues after all tasks completed");
}
