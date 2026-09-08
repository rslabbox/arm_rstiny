use crate::test::{run_allocator_tests};

pub fn user_main() {
    run_allocator_tests();

    info!(""); // Empty line separator
    info!("Starting file system performance tests...");
    // run_fatfs_performance_tests();
    // virtio_test();
}
