//! Performance tests: compute-intensive single-core vs multi-core benchmark.
//!
//! This module measures how well the kernel utilizes multiple cores for
//! CPU-bound workloads.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::config::kernel::MAX_CPUS;
use crate::drivers::timer::current_nanoseconds;
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

    // Busy wait for a bit to give scheduler time
    let start = current_nanoseconds();
    let mut counter = 0u64;
    while current_nanoseconds() - start < 10_000_000 {
        // 10ms busy loop
        counter += 1;
        if counter % 100000 == 0 {
            // Periodically check CPU (in case of migration)
            let current_cpu = percpu::cpu_id();
            if current_cpu != cpu_id {
                info!(
                    "[Worker] Migrated from CPU {} to CPU {}",
                    cpu_id, current_cpu
                );
            }
        }
    }

    info!(
        "[Worker] Finished on CPU {}, iterations: {}",
        percpu::cpu_id(),
        counter
    );
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

    // Then run the compute benchmark (commented out for now to focus on diagnostic)
    /*
    warn!("\n========== Compute Performance Benchmark ==========");
    info!("CPUs available: {}", MAX_CPUS);
    info!("Workload: fib({}) x {} units", WORK_N, WORK_UNITS);

    // Single-core baseline
    let single_ns = bench_single_core();

    // Multi-core with different worker counts
    let multi_ns_half = bench_multi_core(MAX_CPUS / 2);
    let multi_ns_full = bench_multi_core(MAX_CPUS);
    let multi_ns_over = bench_multi_core(MAX_CPUS * 2);

    // Calculate speedups
    warn!("\n=== Performance Summary ===");
    info!("Single-core time:        {} ms", single_ns / 1_000_000);

    if multi_ns_half > 0 {
        let speedup = single_ns as f64 / multi_ns_half as f64;
        info!(
            "{} workers time:          {} ms  (speedup: {:.2}x)",
            MAX_CPUS / 2,
            multi_ns_half / 1_000_000,
            speedup
        );
    }

    if multi_ns_full > 0 {
        let speedup = single_ns as f64 / multi_ns_full as f64;
        info!(
            "{} workers time:          {} ms  (speedup: {:.2}x)",
            MAX_CPUS,
            multi_ns_full / 1_000_000,
            speedup
        );
    }

    if multi_ns_over > 0 {
        let speedup = single_ns as f64 / multi_ns_over as f64;
        info!(
            "{} workers time:          {} ms  (speedup: {:.2}x)",
            MAX_CPUS * 2,
            multi_ns_over / 1_000_000,
            speedup
        );
    }

    warn!("===================================================");
    */
}
