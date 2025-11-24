//! Test module.

mod allocator;
mod gicv3;

pub fn rstiny_tests() {
    info!("This is an info message for testing.");
    error!("This is an error message for testing.");
    debug!("This is a debug message for testing.");
    trace!("This is a trace message for testing.");
    warn!("This is a warning message for testing.");

    allocator::run_allocator_tests();

    gicv3::gicv3_tests();

    #[cfg(feature = "opi5p")]
    crate::drivers::pci::test_dw_pcie_atu();
}
