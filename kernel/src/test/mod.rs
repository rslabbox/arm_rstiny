//! Opt-in in-kernel tests and debugger-invoked fault probes. No test syscall.
mod allocator;
mod single_core;

#[unsafe(no_mangle)]
static mut SELF_TEST_PASSED: u64 = 0;

pub fn run() {
    single_core::run();
    allocator::run_allocator_tests();
    // Standard log macros must skip argument evaluation when LOG=off.
    if log::max_level() == log::LevelFilter::Off {
        let evaluations = core::cell::Cell::new(0);
        log::error!("{}", {
            evaluations.set(evaluations.get() + 1);
            0
        });
        log::info!("{}", {
            evaluations.set(evaluations.get() + 1);
            0
        });
        assert_eq!(evaluations.get(), 0);
    }
    unsafe { core::ptr::addr_of_mut!(SELF_TEST_PASSED).write_volatile(1) };
}

// Keep probes out of production and retain them for GDB-driven execution.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_brk() -> ! {
    core::arch::naked_asm!("brk #0x123", "b .");
}
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_write_text() -> ! {
    core::arch::naked_asm!("ldr x0, =_start", "str xzr, [x0]", "b .");
}
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_execute_stack() -> ! {
    core::arch::naked_asm!("sub x0, sp, #16", "br x0", "b .");
}
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_read_guard() -> ! {
    core::arch::naked_asm!("ldr x0, =stack_guard", "ldr x1, [x0]", "b .");
}
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_read_unmapped() -> ! {
    core::arch::naked_asm!("mov x0, #0", "ldr x1, [x0]", "b .");
}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_panic() -> ! {
    panic!("Kernel injected panic");
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.probe")]
extern "C" fn probe_panic_locked() -> ! {
    crate::utils::logging::panic_with_lock_held()
}
