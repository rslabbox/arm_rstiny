//! Test module.
#![allow(unused)]

mod allocator;
mod gicv3;
// mod perf;
mod task;

fn logger_test() {
    warn!("\n=== Logger Test Start ===");

    error!("This is an error message.");
    warn!("This is a warning message.");
    info!("This is an info message.");
    debug!("This is a debug message.");
    trace!("This is a trace message.");
}

pub fn rstiny_tests() {
    logger_test();

    allocator::run_allocator_tests();

    gicv3::gicv3_tests();

    // // Run scheduler tests
    task::run_scheduler_tests();

    // // Run performance tests (single-core and multi-core)
    // perf::run_perf_tests();

    #[cfg(feature = "opi5p")]
    crate::drivers::pci::test_dw_pcie_atu();
}
