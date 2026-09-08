//! Initial address space and EL0 entry. Kernel mappings remain supervisor-only.
use super::{
    PageTableEntry, TrapFrame,
    boot::{Table, prepare_user_tables},
};
use crate::config::{MemFlags, PAGE_SIZE};
use aarch64_cpu::{asm::barrier, registers::*};
use alloc::alloc::{Layout, alloc_zeroed};
use core::ptr::{addr_of, addr_of_mut};
use kernel_abi::{IMAGE_START, STACK_END};
use memory_addr::PhysAddr;

static mut USER_ROOT: Table = Table::EMPTY;
static mut USER_L1: Table = Table::EMPTY;
static mut USER_L2: Table = Table::EMPTY;
static mut USER_L3: Table = Table::EMPTY;

pub unsafe fn init() -> u64 {
    unsafe {
        prepare_user_tables(
            addr_of_mut!(USER_ROOT),
            addr_of_mut!(USER_L1),
            addr_of_mut!(USER_L2),
            addr_of_mut!(USER_L3),
        );
    }
    addr_of!(USER_ROOT) as u64
}

/// Allocate from the kernel's bounded bootstrap heap. Capability-based object
/// allocation will replace this when user-controlled resource creation exists.
pub unsafe fn map_page(va: u64, flags: MemFlags) -> *mut u8 {
    assert!((IMAGE_START..STACK_END).contains(&va) && va.is_multiple_of(PAGE_SIZE as u64));
    let index = ((va >> 12) & 511) as usize;
    let ptr = unsafe { alloc_zeroed(Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap()) };
    assert!(!ptr.is_null(), "root task bootstrap memory exhausted");
    unsafe {
        addr_of_mut!(USER_L3.0)
            .cast::<PageTableEntry>()
            .add(index)
            .write(PageTableEntry::new_page(
                PhysAddr::from_usize(ptr as usize),
                flags | MemFlags::USER,
                false,
            ));
    }
    ptr
}

pub unsafe fn activate(root: u64) {
    // Newly copied executable bytes must reach PoU before EL0 fetches them.
    // Clean the kernel heap mappings using CTR_EL0's D-cache line size.
    unsafe extern "C" {
        static __heap_start: u8;
        static __heap_end: u8;
    }
    let ctr: u64;
    unsafe { core::arch::asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let line_size = 4usize << ((ctr >> 16) & 15);
    for address in
        (addr_of!(__heap_start) as usize..addr_of!(__heap_end) as usize).step_by(line_size)
    {
        unsafe { core::arch::asm!("dc cvau, {0}", in(reg) address, options(nostack)) };
    }
    barrier::dsb(barrier::ISH);
    unsafe { core::arch::asm!("ic iallu", options(nostack)) };
    barrier::dsb(barrier::ISH);
    barrier::isb(barrier::SY);
    TTBR0_EL1.set(root);
    super::instructions::flush_tlb_all();
}

/// Register restore is the unavoidable assembly boundary; all setup is Rust.
/// Reset the EL1 stack before eret, abandoning boot-time Rust call frames.
#[unsafe(naked)]
pub unsafe extern "C" fn enter(frame: *const TrapFrame) -> ! {
    core::arch::naked_asm!(
        "mov x30, x0",
        "ldr x9, =boot_stack_top",
        "mov sp, x9",
        "ldp x9, x10, [x30, #248]",
        "ldr x11, [x30, #264]",
        "msr sp_el0, x9",
        "msr elr_el1, x10",
        "msr spsr_el1, x11",
        "ldp x0, x1, [x30, #0]",
        "ldp x2, x3, [x30, #16]",
        "ldp x4, x5, [x30, #32]",
        "ldp x6, x7, [x30, #48]",
        "ldp x8, x9, [x30, #64]",
        "ldp x10, x11, [x30, #80]",
        "ldp x12, x13, [x30, #96]",
        "ldp x14, x15, [x30, #112]",
        "ldp x16, x17, [x30, #128]",
        "ldp x18, x19, [x30, #144]",
        "ldp x20, x21, [x30, #160]",
        "ldp x22, x23, [x30, #176]",
        "ldp x24, x25, [x30, #192]",
        "ldp x26, x27, [x30, #208]",
        "ldp x28, x29, [x30, #224]",
        "ldr x30, [x30, #240]",
        "eret",
    );
}
