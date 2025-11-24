//! Task system test module.

use crate::task;

/// Test task 1: Highest priority
fn task1() -> ! {
    for i in 0..5 {
        info!("Task 1 (Priority 0) - Count: {}", i);
        task::yield_now();
    }
    info!("Task 1 exiting");
    task::exit();
}

/// Test task 2: Medium priority
fn task2() -> ! {
    for i in 0..5 {
        info!("Task 2 (Priority 5) - Count: {}", i);
        task::yield_now();
    }
    info!("Task 2 exiting");
    task::exit();
}

/// Test task 3: Low priority
fn task3() -> ! {
    for i in 0..10 {
        info!("Task 3 (Priority 10) - Count: {}", i);
        task::yield_now();
    }
    info!("Task 3 exiting");
    task::exit();
}

/// Test task 4: Same priority group (priority 15)
fn same_priority_task_a() -> ! {
    for i in 0..3 {
        info!("Same Priority Task A (Priority 15) - Count: {}", i);
        task::yield_now();
    }
    info!("Same Priority Task A exiting");
    task::exit();
}

/// Test task 5: Same priority group (priority 15)
fn same_priority_task_b() -> ! {
    for i in 0..3 {
        info!("Same Priority Task B (Priority 15) - Count: {}", i);
        task::yield_now();
    }
    info!("Same Priority Task B exiting");
    task::exit();
}

/// Test task 6: Same priority group (priority 15)
fn same_priority_task_c() -> ! {
    for i in 0..3 {
        info!("Same Priority Task C (Priority 15) - Count: {}", i);
        task::yield_now();
    }
    info!("Same Priority Task C exiting");
    task::exit();
}

/// Run task system tests
pub fn run_task_tests() {
    info!("=== Starting Task System Tests ===");

    // Test 1: Priority scheduling
    info!("\n--- Test 1: Priority Scheduling ---");
    info!("Creating tasks with different priorities (0, 5, 10)");

    task::spawn(task1, 0, None).expect("Failed to spawn task1");
    task::spawn(task2, 5, None).expect("Failed to spawn task2");
    task::spawn(task3, 10, None).expect("Failed to spawn task3");

    info!("Tasks created, waiting for execution...");
    // Use spin loop to allow timer interrupts to trigger scheduling
    for _ in 0..30000000 {
        core::hint::spin_loop();
    }

    // Test 2: Same priority round-robin
    info!("\n--- Test 2: Round-Robin for Same Priority ---");
    info!("Creating 3 tasks with same priority (15)");

    task::spawn(same_priority_task_a, 15, None).expect("Failed to spawn same_priority_task_a");
    task::spawn(same_priority_task_b, 15, None).expect("Failed to spawn same_priority_task_b");
    task::spawn(same_priority_task_c, 15, None).expect("Failed to spawn same_priority_task_c");

    info!("Tasks created, waiting for execution...");
    for _ in 0..30000000 {
        core::hint::spin_loop();
    }

    info!("\n=== Task System Tests Completed ===\n");
}
