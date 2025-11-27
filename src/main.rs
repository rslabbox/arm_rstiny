//! RstinyOS - Main kernel entry point.

#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

mod boot;
mod config;

mod console;
mod error;

mod drivers;
mod hal;
mod mm;
mod platform;
mod task;
mod tests;

// Future modules (placeholder)
mod fs;

#[cfg(feature = "net")]
mod net;
mod sync;

#[macro_use]
extern crate log;

extern crate alloc;
extern crate axbacktrace;

use core::sync::atomic::{AtomicUsize, Ordering};

pub use error::{TinyError, TinyResult};
use memory_addr::pa;

use crate::mm::phys_to_virt;
use drivers::timer;

/// Number of secondary CPUs that have completed initialization.
static CPUS_READY: AtomicUsize = AtomicUsize::new(0);

/// Flag to signal secondary CPUs to start scheduling.
static START_SCHEDULING: AtomicUsize = AtomicUsize::new(0);

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

fn kernel_init() {
    hal::clear_bss();
    hal::init_exception();
    drivers::uart::init_early(phys_to_virt(pa!(config::UART_PADDR)));

    // Print build time
    println!(
        "\nBuild time: {}",
        option_env!("BUILD_TIME").unwrap_or("unknown")
    );

    println!("Board: {}", config::BOARD_NAME);

    console::init_logger().expect("Failed to initialize logger");

    backtrace_init();

    drivers::irq::init(
        phys_to_virt(pa!(config::GICD_BASE)),
        phys_to_virt(pa!(config::GICR_BASE)),
    )
    .expect("Failed to initialize IRQ");

    timer::init_early();
    drivers::power::init("hvc").expect("Failed to initialize PSCI");

    // Initialize task scheduler
    task::init_taskmanager();
}

/// User main task entry point.
fn user_main() {
    // Run tests in main task
    tests::rstiny_tests();

    // Run scheduler tests
    tests::task::run_scheduler_tests();

    info!("User main task completed");
}

/// Boot all secondary CPUs using PSCI.
fn boot_secondary_cpus() {
    use crate::config::kernel::MAX_CPUS;

    let entry_paddr = boot::secondary_entry_paddr();

    for cpu_id in 1..MAX_CPUS {
        info!("Starting CPU {}...", cpu_id);
        drivers::power::cpu_on(cpu_id, entry_paddr, cpu_id);
    }

    // Wait for all secondary CPUs to be ready
    info!("Waiting for {} secondary CPUs...", MAX_CPUS - 1);
    while CPUS_READY.load(Ordering::SeqCst) < MAX_CPUS - 1 {
        core::hint::spin_loop();
    }
    info!("All {} CPUs online", MAX_CPUS);
}

#[unsafe(no_mangle)]
pub fn rust_main(_cpu_id: usize, _arg: usize) -> ! {
    kernel_init();

    println!("\nHello RustTinyOS!\n");

    // Boot secondary CPUs
    boot_secondary_cpus();

    // Create main user task as child of ROOT
    task::spawn(user_main);

    // Signal all CPUs to start scheduling
    START_SCHEDULING.store(1, Ordering::SeqCst);

    // Start scheduler, transfer control to ROOT
    task::start_scheduling();
}

/// Secondary CPU entry point (called from assembly).
///
/// This function is called by each secondary CPU after basic hardware
/// initialization (EL switch, FP enable, MMU setup).
#[unsafe(no_mangle)]
pub fn rust_main_secondary(cpu_id: usize) -> ! {
    // Initialize per-CPU data
    unsafe {
        hal::percpu::init(cpu_id);
    }

    // Initialize GIC for this CPU
    drivers::irq::init_secondary(cpu_id);

    // Initialize timer for this CPU
    drivers::timer::init_secondary();

    info!("CPU {} online", cpu_id);

    // Signal that this CPU is ready
    CPUS_READY.fetch_add(1, Ordering::SeqCst);

    // Wait for primary CPU to signal start
    while START_SCHEDULING.load(Ordering::SeqCst) == 0 {
        core::hint::spin_loop();
    }

    // Enter idle loop
    // TODO: In the future, secondary CPUs will join the scheduler
    info!("CPU {} entering idle loop", cpu_id);
    loop {
        aarch64_cpu::asm::wfi();
    }
}

#[cfg(all(target_os = "none", not(test)))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    error!("{}", info);

    // Capture and display backtrace
    error!("\n{}", axbacktrace::Backtrace::capture());

    drivers::power::system_off();
}
