//! Kernel configuration constants.
use arm_gic::IntId;
use const_env::env_item;
use core::sync::atomic::{AtomicUsize, Ordering};

pub const BOOT_STACK_SIZE: usize = 0x40000;

/// Kernel image virtual address (link address).
/// This is the virtual address where the kernel is linked.
/// The kernel uses 2MB block mapping at L2 level for position-independent loading.
#[env_item]
pub const TINYENV_KIMAGE_VADDR: usize = 0xffff_0000_8000_0000;

/// Runtime kernel physical base address.
/// This is set during early boot when the actual load address is determined.
static KERNEL_PHYS_BASE: AtomicUsize = AtomicUsize::new(0);

/// Get the kernel physical base address.
/// Returns the physical address where the kernel was actually loaded.
#[inline]
pub fn kernel_phys_base() -> usize {
    KERNEL_PHYS_BASE.load(Ordering::Relaxed)
}

/// Set the kernel physical base address.
/// Called once during early boot after determining the actual load address.
///
/// # Safety
/// This should only be called once during early boot initialization.
#[inline]
pub fn set_kernel_phys_base(paddr: usize) {
    KERNEL_PHYS_BASE.store(paddr, Ordering::Relaxed);
}

pub const HEAP_ALLOCATOR_SIZE: usize = 0x1000000; // 16MB
pub const TICKS_PER_SEC: usize = 100; // 100 ticks per second (1ms per tick)

// Multi-core configuration
pub const SECONDARY_STACK_SIZE: usize = 0x10000; // 64KB per secondary CPU

// Task scheduling configuration
pub const TASK_STACK_SIZE: usize = 0x10000; // 64KB per task

// Timer interrupt configuration
pub const TIMER_IRQ: IntId = IntId::ppi(14);

#[env_item]
pub const TINYENV_SMP: usize = 1;

const _: () = assert!(TINYENV_SMP < 16, "TINYENV_SMP must be less than 16");

#[env_item]
pub const TINYENV_LOG: &str = "warn";

#[env_item]
pub const TINYENV_BUILD_TIME: &str = "unknown";
