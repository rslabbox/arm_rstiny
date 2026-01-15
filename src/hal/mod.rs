//! HAL (Hardware Abstraction Layer) module.
//!
//! This module provides hardware abstractions for the AArch64 architecture,
//! including CPU operations, MMU management, and exception handling.

pub mod context;
pub mod cpu;
pub mod exception;
pub mod percpu;
mod spin;

pub use context::TrapFrame;
pub use cpu::{clear_bss, flush_tlb};
pub use exception::init_exception;
pub use spin::{Mutex, SpinNoIrq};