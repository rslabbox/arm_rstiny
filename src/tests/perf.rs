//! Multi-core CPU compute-intensive performance tests.
//!
//! Uses matrix multiplication to benchmark single-core vs multi-core performance.

use alloc::vec::Vec;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

use crate::config::kernel::TINYENV_SMP;
use crate::drivers::timer::busy_wait;
use crate::drivers::timer::generic_timer::current_nanoseconds;
use crate::hal::percpu::cpu_id;
use crate::task::thread;

/// Matrix size for the benchmark (N x N)
const MATRIX_SIZE: usize = 200;

/// Number of iterations for the benchmark
const ITERATIONS: usize = 50;

/// A simple N x N matrix of i64 integers
struct Matrix {
    data: Vec<i64>,
    size: usize,
}

impl Matrix {
    /// Create a new zero-initialized matrix
    fn new(size: usize) -> Self {
        Self {
            data: alloc::vec![0i64; size * size],
            size,
        }
    }

    /// Create a matrix filled with random values using the provided RNG
    fn random(size: usize, rng: &mut SmallRng) -> Self {
        let mut matrix = Self::new(size);
        for i in 0..size * size {
            matrix.data[i] = rng.random_range(-100..100);
        }
        matrix
    }

    /// Get element at (row, col)
    #[inline]
    fn get(&self, row: usize, col: usize) -> i64 {
        self.data[row * self.size + col]
    }

    /// Set element at (row, col)
    #[inline]
    fn set(&mut self, row: usize, col: usize, value: i64) {
        self.data[row * self.size + col] = value;
    }

    /// Multiply two matrices: self * other
    fn multiply(&self, other: &Matrix) -> Matrix {
        assert_eq!(self.size, other.size);
        let n = self.size;
        let mut result = Matrix::new(n);

        for i in 0..n {
            for j in 0..n {
                let mut sum: i64 = 0;
                for k in 0..n {
                    sum = sum.wrapping_add(self.get(i, k).wrapping_mul(other.get(k, j)));
                }
                result.set(i, j, sum);
            }
        }
        result
    }
}

/// Perform matrix multiplication benchmark for a given number of iterations
/// Returns the total time in nanoseconds
fn matrix_benchmark(iterations: usize, seed: u64) -> u64 {
    let mut rng = SmallRng::seed_from_u64(seed);

    let start = current_nanoseconds();

    for _ in 0..iterations {
        let a = Matrix::random(MATRIX_SIZE, &mut rng);
        let b = Matrix::random(MATRIX_SIZE, &mut rng);
        let _c = a.multiply(&b);
        // Use black_box equivalent to prevent optimization
        core::hint::black_box(&_c);
    }

    let end = current_nanoseconds();
    end - start
}

/// Run single-core benchmark
fn bench_single_core() -> u64 {
    info!("=== Single-Core Benchmark ===");
    info!(
        "Matrix size: {}x{}, Iterations: {}",
        MATRIX_SIZE, MATRIX_SIZE, ITERATIONS
    );

    let time_ns = matrix_benchmark(ITERATIONS, 42);
    let time_ms = time_ns / 1_000_000;

    info!("Single-core time: {} ms ({} ns)", time_ms, time_ns);
    time_ns
}

/// Run multi-core benchmark
fn bench_multi_core() -> u64 {
    let num_cpus = TINYENV_SMP;
    let iterations_per_cpu = ITERATIONS / num_cpus;
    let remainder = ITERATIONS % num_cpus;

    info!("=== Multi-Core Benchmark ===");
    info!(
        "Matrix size: {}x{}, Total iterations: {}, CPUs: {}",
        MATRIX_SIZE, MATRIX_SIZE, ITERATIONS, num_cpus
    );
    info!(
        "Iterations per CPU: {} (remainder: {})",
        iterations_per_cpu, remainder
    );

    let start = current_nanoseconds();

    // Spawn worker tasks on each CPU
    let mut handles = Vec::new();

    for i in 0..num_cpus {
        // Distribute remainder iterations to first few CPUs
        let iters = if i < remainder {
            iterations_per_cpu + 1
        } else {
            iterations_per_cpu
        };

        // Each worker gets a unique seed based on its index
        let seed = 42u64 + i as u64;

        let handle = thread::spawn(move || {
            let cpu = cpu_id();
            debug!("[Worker {}] Starting on CPU {}, iterations: {}", i, cpu, iters);

            let time_ns = matrix_benchmark(iters, seed);

            debug!(
                "[Worker {}] Finished on CPU {}, time: {} ms",
                i,
                cpu,
                time_ns / 1_000_000
            );
            time_ns
        });

        handles.push(handle);
    }

    // Wait for all workers to complete
    let mut worker_times = Vec::new();
    for (i, handle) in handles.into_iter().enumerate() {
        match handle.join() {
            Ok(time) => {
                worker_times.push(time);
                debug!("Worker {} joined, time: {} ms", i, time / 1_000_000);
            }
            Err(e) => {
                warn!("Worker {} failed: {:?}", i, e);
            }
        }
    }

    let end = current_nanoseconds();
    let total_time = end - start;
    let total_time_ms = total_time / 1_000_000;

    info!("Multi-core total time: {} ms ({} ns)", total_time_ms, total_time);

    // Print individual worker times
    for (i, time) in worker_times.iter().enumerate() {
        debug!("  Worker {}: {} ms", i, time / 1_000_000);
    }

    total_time
}

/// Run all performance tests and print summary
pub fn run_perf_tests() {
    info!("========== Multi-Core Performance Benchmark ==========");
    info!("Current task running on CPU {}", cpu_id());

    // Run single-core benchmark
    let single_time = bench_single_core();

    // Small delay to ensure clean separation
    busy_wait(core::time::Duration::from_millis(100));

    // Run multi-core benchmark
    let multi_time = bench_multi_core();

    // Calculate and print results
    info!("========== Performance Summary ==========");
    info!(
        "Single-core time: {} ms",
        single_time / 1_000_000
    );
    info!(
        "Multi-core time:  {} ms",
        multi_time / 1_000_000
    );

    if multi_time > 0 && single_time > 0 {
        // Calculate speedup: single_time / multi_time
        // Using integer math: (single_time * 100) / multi_time gives speedup * 100
        let speedup_x100 = (single_time * 100) / multi_time;
        let speedup_int = speedup_x100 / 100;
        let speedup_frac = speedup_x100 % 100;

        info!(
            "Speedup: {}.{:02}x ({} CPUs)",
            speedup_int, speedup_frac, TINYENV_SMP
        );

        // Calculate efficiency: (speedup / num_cpus) * 100
        // efficiency = (single_time * 100) / (multi_time * num_cpus)
        let efficiency = (single_time * 100) / (multi_time * TINYENV_SMP as u64);

        info!("Parallel efficiency: {}%", efficiency);

        // Expected ideal speedup
        info!("Ideal speedup: {}x (100% efficiency)", TINYENV_SMP);
    }

    info!("==========================================");
}
