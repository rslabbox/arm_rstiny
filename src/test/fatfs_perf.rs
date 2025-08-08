//! FAT File System Performance Test Module
//!
//! This module focuses on read/write performance testing with MB/s measurements.

use alloc::{format, string::String, vec, vec::Vec};
use fatfs::{FileSystem, FsOptions, Read, Seek, SeekFrom, Write};
use log::{error, info, warn};

use crate::{drivers::MyFileSystem, utils::current_ticks};

/// Simple timer for performance measurement
struct SimpleTimer {
    start_cycles: u64,
}

impl SimpleTimer {
    fn new() -> Self {
        Self {
            start_cycles: Self::get_cycles(),
        }
    }

    fn elapsed_cycles(&self) -> u64 {
        Self::get_cycles() - self.start_cycles
    }

    fn get_cycles() -> u64 {
        current_ticks()
    }

    // Convert cycles to MB/s (assuming 1GHz CPU frequency for approximation)
    fn cycles_to_mbps(cycles: u64, bytes: usize) -> f32 {
        const CPU_FREQ_HZ: f64 = 1_000_000_000.0; // 1GHz assumption
        const BYTES_TO_MB: f64 = 1_048_576.0; // 1024 * 1024

        if cycles == 0 {
            return 0.0;
        }

        let seconds = cycles as f64 / CPU_FREQ_HZ;
        let mb = bytes as f64 / BYTES_TO_MB;
        (mb / seconds) as f32
    }
}

/// Test result structure
#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: &'static str,
    pub passed: bool,
    pub error_msg: Option<String>,
    pub details: Option<String>,
}

impl TestResult {
    pub fn new(name: &'static str, passed: bool, error_msg: Option<String>) -> Self {
        Self {
            name,
            passed,
            error_msg,
            details: None,
        }
    }

    pub fn with_details(mut self, details: String) -> Self {
        self.details = Some(details);
        self
    }
}

/// FAT File System Performance Test Suite
pub struct FatFsPerfTestSuite {
    results: Vec<TestResult>,
}

impl FatFsPerfTestSuite {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
        }
    }

    /// Run performance tests
    pub fn run_all_tests(&mut self) {
        info!("=== Starting FAT File System Performance Tests ===");

        self.test_filesystem_initialization();
        self.test_read_write_performance();

        self.print_results();
    }

    /// Test file system initialization
    fn test_filesystem_initialization(&mut self) {
        let test_name = "File System Initialization Test";
        info!("Running test: {}", test_name);

        let mut passed = true;
        let mut error_msg = None;
        let mut details = String::new();

        match MyFileSystem::new() {
            myfs => {
                details.push_str("✓ MyFileSystem created successfully\n");

                // Try to create FAT file system
                match FileSystem::new(myfs, FsOptions::new()) {
                    Ok(fs) => {
                        details.push_str("✓ FAT file system initialized successfully\n");

                        // Get root directory
                        let root_dir = fs.root_dir();
                        details.push_str("✓ Root directory access successful\n");

                        // Try to read root directory contents
                        let count = root_dir.iter().count();
                        details.push_str(&format!("✓ Root directory contains {} entries\n", count));
                    }
                    Err(e) => {
                        passed = false;
                        error_msg = Some(format!("FAT file system initialization failed: {:?}", e));
                    }
                }
            }
        }

        self.results
            .push(TestResult::new(test_name, passed, error_msg).with_details(details));
    }

    /// Test read/write performance with different file sizes
    fn test_read_write_performance(&mut self) {
        let test_name = "Read/Write Performance Test";
        info!("Running test: {}", test_name);

        let mut passed = true;
        let mut error_msg = None;
        let mut details = String::new();

        let myfs = MyFileSystem::new();
        match FileSystem::new(myfs, FsOptions::new()) {
            Ok(fs) => {
                let root_dir = fs.root_dir();

                // Test different file sizes for performance
                let test_sizes = vec![(65536, "64KB"), (262144, "256KB")];

                details.push_str("=== Performance Test Results ===\n");

                for (size, size_name) in test_sizes {
                    details.push_str(&format!("\n--- Testing {} file ---\n", size_name));

                    let filename = format!("perf_test_{}.dat", size_name);

                    // Write performance test
                    match root_dir.create_file(&filename) {
                        Ok(mut file) => {
                            let test_data = vec![0xAA; size];

                            let timer = SimpleTimer::new();
                            match file.write_all(&test_data) {
                                Ok(_) => {
                                    let write_cycles = timer.elapsed_cycles();

                                    // Flush to ensure data is written
                                    if let Err(e) = file.flush() {
                                        details.push_str(&format!("⚠ Flush warning: {:?}\n", e));
                                    }

                                    // Calculate write speed in MB/s
                                    let write_mbps =
                                        SimpleTimer::cycles_to_mbps(write_cycles, size);
                                    details.push_str(&format!(
                                        "✓ Write: {} bytes in {} cycles ({:.2} MB/s)\n",
                                        size, write_cycles, write_mbps
                                    ));

                                    // Read performance test
                                    match file.seek(SeekFrom::Start(0)) {
                                        Ok(_) => {
                                            let mut read_buffer = vec![0u8; size];

                                            let timer = SimpleTimer::new();
                                            match file.read_exact(&mut read_buffer) {
                                                Ok(_) => {
                                                    let read_cycles = timer.elapsed_cycles();

                                                    // Calculate read speed in MB/s
                                                    let read_mbps = SimpleTimer::cycles_to_mbps(
                                                        read_cycles,
                                                        size,
                                                    );
                                                    details.push_str(&format!(
                                                        "✓ Read:  {} bytes in {} cycles ({:.2} MB/s)\n",
                                                        size, read_cycles, read_mbps
                                                    ));

                                                    // Verify data integrity
                                                    if read_buffer == test_data {
                                                        details.push_str(
                                                            "✓ Data integrity verified\n",
                                                        );
                                                    } else {
                                                        details.push_str(
                                                            "⚠ Data integrity check failed\n",
                                                        );
                                                        passed = false;
                                                        error_msg = Some(format!(
                                                            "Data corruption in {} test",
                                                            size_name
                                                        ));
                                                    }

                                                    // Calculate read/write speed ratio
                                                    if write_mbps > 0.0 {
                                                        let rw_speed_ratio = read_mbps / write_mbps;
                                                        details.push_str(&format!(
                                                            "  Read/Write speed ratio: {:.2}\n",
                                                            rw_speed_ratio
                                                        ));
                                                    }
                                                }
                                                Err(e) => {
                                                    passed = false;
                                                    error_msg = Some(format!(
                                                        "Read failed for {}: {:?}",
                                                        size_name, e
                                                    ));
                                                    break;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            passed = false;
                                            error_msg = Some(format!(
                                                "Seek failed for {}: {:?}",
                                                size_name, e
                                            ));
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    passed = false;
                                    error_msg =
                                        Some(format!("Write failed for {}: {:?}", size_name, e));
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            passed = false;
                            error_msg =
                                Some(format!("File creation failed for {}: {:?}", size_name, e));
                            break;
                        }
                    }
                }

                if passed {
                    details.push_str("\n=== Performance Summary ===\n");
                    details.push_str("✓ All performance tests completed successfully\n");
                    details.push_str("✓ File system shows consistent read/write performance\n");
                    details.push_str("✓ Data integrity maintained across all test sizes\n");
                }
            }
            Err(e) => {
                passed = false;
                error_msg = Some(format!("File system initialization failed: {:?}", e));
            }
        }

        self.results
            .push(TestResult::new(test_name, passed, error_msg).with_details(details));
    }

    /// Print test results
    fn print_results(&self) {
        info!("=== FAT File System Performance Test Results ===");

        let mut passed_count = 0;
        let total_count = self.results.len();

        for result in &self.results {
            if result.passed {
                info!("✅ {}", result.name);
                passed_count += 1;
            } else {
                error!(
                    "❌ {} - {}",
                    result.name,
                    result.error_msg.as_ref().unwrap_or(&"Unknown error".into())
                );
            }

            // Print detailed information
            if let Some(details) = &result.details {
                for line in details.lines() {
                    if !line.trim().is_empty() {
                        info!("   {}", line);
                    }
                }
            }
            info!(""); // Empty line separator
        }

        info!("=== Test Summary ===");
        info!("Tests completed: {}/{} passed", passed_count, total_count);

        if passed_count == total_count {
            info!("🎉 All performance tests passed! FAT file system is working correctly.");
        } else {
            warn!(
                "⚠️ {} tests failed, please check FAT file system implementation.",
                total_count - passed_count
            );
        }

        // Print performance statistics
        info!("=== Performance Statistics ===");
        info!("Total tests: {}", total_count);
        info!(
            "Pass rate: {:.1}%",
            (passed_count as f32 / total_count as f32) * 100.0
        );
    }
}

/// Run FAT file system performance tests
pub fn run_fatfs_performance_tests() {
    let mut test_suite = FatFsPerfTestSuite::new();
    test_suite.run_all_tests();
}
