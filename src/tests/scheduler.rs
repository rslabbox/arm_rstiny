//! Task scheduler tests.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use crate::drivers::timer::{busy_wait, current_ticks, ticks_to_nanos, NANOS_PER_MILLIS};
use crate::task::thread;

/// Test basic task spawning.
fn test_basic_spawn() {
    info!("=== Test: Basic Spawn ===");

    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = alloc::vec::Vec::new();

    // Spawn 3 tasks
    for i in 0..3 {
        let c = counter.clone();
        let handle = thread::spawn(move || {
            info!("Task {} started", i);
            c.fetch_add(1, Ordering::SeqCst);
            info!("Task {} finished", i);
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.join();
    }

    let count = counter.load(Ordering::SeqCst);
    info!("Counter value: {}", count);

    if count == 3 {
        info!("✓ Basic spawn test PASSED");
    } else {
        error!("✗ Basic spawn test FAILED: expected 3, got {}", count);
    }
}

/// Test sleep functionality.
fn test_sleep() {
    info!("=== Test: Sleep ===");

    let handle = thread::spawn(|| {
        info!("Task: Sleeping for 50ms");
        let start = current_ticks();
        thread::sleep(Duration::from_millis(50));
        let end = current_ticks();

        let elapsed_ns = ticks_to_nanos(end - start);
        let elapsed_ms = elapsed_ns / NANOS_PER_MILLIS;

        info!("Task: Woke up after {} ms", elapsed_ms);

        if elapsed_ms >= 50 && elapsed_ms < 60 {
            info!("✓ Sleep test PASSED");
        } else {
            error!("✗ Sleep test FAILED: expected ~50ms, got {}ms", elapsed_ms);
        }
    });

    // Wait for test to complete
    handle.join();
}

/// Test multiple concurrent tasks.
fn test_multiple_tasks() {
    info!("=== Test: Multiple Tasks ===");

    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = alloc::vec::Vec::new();

    // Spawn 5 tasks that increment counter
    for i in 0..5 {
        let c = counter.clone();
        let handle = thread::spawn(move || {
            for j in 0..10 {
                c.fetch_add(1, Ordering::SeqCst);
                if j % 3 == 0 {
                    info!("Task {} iteration {}", i, j);
                }
                // Small delay
                busy_wait(Duration::from_micros(100));
            }
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.join();
    }

    let count = counter.load(Ordering::SeqCst);
    info!("Final counter value: {}", count);

    if count == 50 {
        info!("✓ Multiple tasks test PASSED");
    } else {
        error!("✗ Multiple tasks test FAILED: expected 50, got {}", count);
    }
}

/// Test yield functionality (simplified).
fn test_yield() {
    info!("=== Test: Yield ===");

    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = alloc::vec::Vec::new();

    for i in 0..2 {
        let c = counter.clone();
        let handle = thread::spawn(move || {
            for _ in 0..5 {
                c.fetch_add(1, Ordering::SeqCst);
                thread::yield_now();
            }
            info!("Task {} completed", i);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join();
    }

    let count = counter.load(Ordering::SeqCst);
    info!("Counter value: {}", count);

    if count == 10 {
        info!("✓ Yield test PASSED");
    } else {
        warn!("✗ Yield test INCOMPLETE: expected 10, got {}", count);
    }
}

/// Test task with computation (time slice).
fn test_time_slice() {
    info!("=== Test: Time Slice ===");

    let executed = Arc::new(AtomicUsize::new(0));
    let mut handles = alloc::vec::Vec::new();

    // Spawn CPU-intensive tasks
    for i in 0..3 {
        let e = executed.clone();
        let handle = thread::spawn(move || {
            info!("Task {} starting computation", i);
            // Simulate work
            for _ in 0..1000 {
                e.fetch_add(1, Ordering::Relaxed);
            }
            info!("Task {} finished", i);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join();
    }

    let total = executed.load(Ordering::Relaxed);
    info!("Total executions: {}", total);

    if total >= 3000 {
        info!("✓ Time slice test PASSED");
    } else {
        warn!("✗ Time slice test INCOMPLETE: expected 3000, got {}", total);
    }
}

/// Run all scheduler tests.
pub fn run_scheduler_tests() {
    info!("\n=== Running Task Scheduler Tests ===\n");

    test_basic_spawn();

    test_multiple_tasks();

    test_yield();

    test_sleep();

    test_time_slice();

    info!("\n=== Task Scheduler Tests Complete ===\n");
}
