//! 测试模块
//!
//! 这个模块包含了系统各个组件的测试代码

pub mod allocator;
pub mod fatfs_perf;

pub use allocator::run_allocator_tests;
pub use fatfs_perf::run_fatfs_performance_tests;
