//! Performance tests: compute-intensive single-core vs multi-core benchmark.
//!
//! This module measures how well the kernel utilizes multiple cores for
//! CPU-bound workloads.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

use crate::config::kernel::MAX_CPUS;
use crate::drivers::timer::busy_wait;
use crate::hal::percpu;
use crate::task::thread;

// ---------------------------------------------------------------------------
// Multi-core diagnostic test
// ---------------------------------------------------------------------------

/// Per-CPU counter to track which CPUs actually ran tasks
static CPU_RUN_COUNT: [AtomicUsize; 8] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// Simple busy-wait worker that reports its CPU
fn cpu_diagnostic_worker() {
    let cpu_id = percpu::cpu_id();

    // Increment the counter for this CPU
    CPU_RUN_COUNT[cpu_id].fetch_add(1, Ordering::Relaxed);

    info!("[Worker] Running on CPU {}", cpu_id);

    busy_wait(Duration::from_secs(1));

    info!("[Worker] Finished on CPU {}", percpu::cpu_id(),);
}

/// Test to diagnose multi-core scheduling
pub fn test_multicore_scheduling() {
    warn!("\n========== Multi-core Scheduling Diagnostic ==========");
    info!("MAX_CPUS = {}", MAX_CPUS);
    info!("Current task running on CPU {}", percpu::cpu_id());

    // Reset counters
    for i in 0..8 {
        CPU_RUN_COUNT[i].store(0, Ordering::Relaxed);
    }

    // Spawn 2x MAX_CPUS workers
    let num_workers = MAX_CPUS * 2;
    info!("Spawning {} worker tasks...", num_workers);

    let mut handles = Vec::new();
    for i in 0..num_workers {
        info!("Spawning worker {}", i);
        handles.push(thread::spawn(cpu_diagnostic_worker));
    }

    info!("Waiting for all workers to complete...");
    for (i, h) in handles.into_iter().enumerate() {
        h.join().unwrap();
        info!("Worker {} joined", i);
    }

    // Report which CPUs ran tasks
    warn!("\n=== CPU Usage Summary ===");
    for cpu in 0..MAX_CPUS {
        let count = CPU_RUN_COUNT[cpu].load(Ordering::Relaxed);
        info!("CPU {}: {} tasks ran", cpu, count);
    }
    warn!("==========================================");
}

// ---------------------------------------------------------------------------
// Combined performance report
// ---------------------------------------------------------------------------

pub fn run_perf_tests() {
    // First run the multi-core diagnostic test
    test_multicore_scheduling();
}
