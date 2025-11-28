//! Boot module - Early kernel initialization.
//!
//! This module contains all the code needed to boot the kernel, including:
//! - Assembly entry point with Linux image header
//! - Exception level switching (EL3/EL2 -> EL1)
//! - MMU initialization
//! - Boot page table setup

pub mod entry;
pub mod init;
pub mod mmu;

use core::sync::atomic::{AtomicUsize, Ordering};

use memory_addr::pa;

use crate::{config::kernel::PHYS_VIRT_OFFSET, hal::percpu, mm::phys_to_virt, println};

use crate::config::kernel::TINYENV_SMP;

/// Number of secondary CPUs that have completed initialization.
static CPUS_READY: AtomicUsize = AtomicUsize::new(0);

/// Flag to signal secondary CPUs to start scheduling.
static START_SCHEDULING: AtomicUsize = AtomicUsize::new(0);

/// Initialize backtrace support.
fn backtrace_init() {
    use core::ops::Range;

    unsafe extern "C" {
        safe static _stext: [u8; 0];
        safe static _etext: [u8; 0];
        safe static _edata: [u8; 0];
    }

    let ip_range = Range {
        start: _stext.as_ptr() as usize,
        end: _etext.as_ptr() as usize,
    };

    let fp_range = Range {
        start: _edata.as_ptr() as usize,
        end: usize::MAX,
    };
    axbacktrace::init(ip_range, fp_range);
}

/// Initialize the kernel subsystems.
fn kernel_init() {
    // Clear BSS, initialize exceptions, early UART
    crate::hal::clear_bss();
    crate::hal::init_exception();
    crate::drivers::uart::init_early(phys_to_virt(pa!(crate::config::UART_PADDR)));

    percpu::init(0); // Initialize percpu for CPU 0

    // Print build time
    println!(
        "\nBuild time: {}",
        crate::config::kernel::TINYENV_BUILD_TIME
    );

    println!("Board: {}", crate::config::BOARD_NAME);

    crate::console::init_logger().expect("Failed to initialize logger");

    backtrace_init();

    crate::drivers::irq::init(
        phys_to_virt(pa!(crate::config::GICD_BASE)),
        phys_to_virt(pa!(crate::config::GICR_BASE)),
    )
    .expect("Failed to initialize IRQ");

    crate::drivers::timer::init_early();
    crate::drivers::power::init("hvc").expect("Failed to initialize PSCI");

    // Initialize task scheduler
    crate::task::init_taskmanager();
}

/// Returns the physical address of the secondary CPU entry point.
///
/// This is used by the primary CPU to start secondary CPUs via PSCI cpu_on.
pub fn secondary_entry_paddr() -> usize {
    // The entry point virtual address minus the offset gives the physical address
    entry::_start_secondary as *const () as usize - PHYS_VIRT_OFFSET
}

/// Boot all secondary CPUs using PSCI.
fn boot_secondary_cpus() {
    let entry_paddr = crate::boot::secondary_entry_paddr();

    for cpu_id in 1..TINYENV_SMP {
        info!("Starting CPU {}...", cpu_id);
        crate::drivers::power::cpu_on(cpu_id, entry_paddr, cpu_id);
    }

    // Wait for all secondary CPUs to be ready
    info!("Waiting for {} secondary CPUs...", TINYENV_SMP - 1);
    while CPUS_READY.load(Ordering::SeqCst) < TINYENV_SMP - 1 {
        core::hint::spin_loop();
    }
    info!("All {} CPUs online", TINYENV_SMP);
}

/// Rust main entry point (called from assembly).
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    // Initialize kernel subsystems
    kernel_init();

    println!("\nHello RustTinyOS!\n");

    // Boot secondary CPUs
    boot_secondary_cpus();

    // Create main user task as child of ROOT
    crate::task::spawn(crate::main);

    // Signal all CPUs to start scheduling
    START_SCHEDULING.store(1, Ordering::SeqCst);

    // Start scheduler, transfer control to ROOT
    crate::task::start_scheduling();
}

/// Secondary CPU entry point (called from assembly).
///
/// This function is called by each secondary CPU after basic hardware
/// initialization (EL switch, FP enable, MMU setup).
pub fn rust_main_secondary(cpu_id: usize) -> ! {
    // Initialize percpu for this CPU
    percpu::init(cpu_id);
    crate::hal::init_exception();

    // Initialize task scheduler for this CPU (creates idle task, sets up percpu)
    crate::task::init_taskmanager_secondary(cpu_id);
    
    // Initialize GIC for this CPU
    crate::drivers::irq::init_secondary(cpu_id);

    // Initialize timer for this CPU
    crate::drivers::timer::init_secondary();

    info!("CPU {} online", cpu_id);

    // Signal that this CPU is ready
    CPUS_READY.fetch_add(1, Ordering::SeqCst);

    // Wait for primary CPU to signal start
    while START_SCHEDULING.load(Ordering::SeqCst) == 0 {
        core::hint::spin_loop();
    }

    // Join the scheduler - secondary CPUs participate in task scheduling
    info!("CPU {} joining scheduler", cpu_id);
    crate::task::start_scheduling();
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    error!("{}", info);

    // Capture and display backtrace
    error!("\n{}", axbacktrace::Backtrace::capture());

    crate::drivers::power::system_off();
}
