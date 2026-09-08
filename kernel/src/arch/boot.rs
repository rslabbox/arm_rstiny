//! Fixed, single-core QEMU virt boot with page-granular identity mappings.
//! The low identity layout is temporary; Userspace support must introduce a user TTBR0 layout.
use aarch64_cpu::{asm, asm::barrier, registers::*};
use core::ptr::{addr_of, addr_of_mut};
use memory_addr::PhysAddr;

use super::PageTableEntry;
use crate::config::{MemFlags, PAGE_SIZE, RAM_START};

// Cortex-A72 reset baseline: retain the architecturally required RES1 bits.
const SCTLR_RESET: u64 = 0x30d0_0800;

/// Only stackless entry work remains in assembly. A Rust prologue must not
/// run until secondary CPUs are parked and the boot stack is established.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "msr daifset, #0xf",
        "mrs x0, mpidr_el1",
        "ldr x1, =0xff00ffffff", // Aff3/Aff2/Aff1/Aff0, ignoring other MPIDR bits
        "tst x0, x1",
        "b.ne 2f",
        "msr spsel, #1",
        "ldr x0, =boot_stack_top",
        "mov sp, x0",
        "b {start}",
        "2:",
        "wfe",
        "b 2b",
        start = sym start_rust,
    );
}

extern "C" fn start_rust() -> ! {
    match CurrentEL.read(CurrentEL::EL) {
        1 => init_el1(1),
        2 => switch_to_el1(),
        // Secure/EL3 entry is outside the QEMU platform contract.
        _ => crate::utils::halt(),
    }
}

fn switch_to_el1() -> ! {
    HCR_EL2.write(HCR_EL2::RW::EL1IsAarch64);
    CNTHCTL_EL2.write(CNTHCTL_EL2::EL1PCEN::SET + CNTHCTL_EL2::EL1PCTEN::SET);
    CNTVOFF_EL2.set(0);
    CPTR_EL2.set(0);
    SCTLR_EL1.set(SCTLR_RESET);
    SPSR_EL2.write(
        SPSR_EL2::M::EL1h
            + SPSR_EL2::D::Masked
            + SPSR_EL2::A::Masked
            + SPSR_EL2::I::Masked
            + SPSR_EL2::F::Masked,
    );
    SP_EL1.set(addr_of!(boot_stack_top) as u64);
    ELR_EL2.set(el1_entry as *const () as u64);
    asm::eret()
}

/// Discard the EL2 Rust frames rather than returning through them at EL1.
/// Resetting SP inside an ordinary Rust function would invalidate its frame.
#[unsafe(naked)]
unsafe extern "C" fn el1_entry() -> ! {
    core::arch::naked_asm!(
        "ldr x1, =boot_stack_top",
        "mov sp, x1",
        "mov x0, #2",
        "b {init}",
        init = sym init_el1,
    );
}

extern "C" fn init_el1(entry_el: u64) -> ! {
    VBAR_EL1.set(exception_vector_base as *const () as u64);
    barrier::isb(barrier::SY);
    // The cold-boot contract requires MMU and caches off.
    if SCTLR_EL1.is_set(SCTLR_EL1::M)
        || SCTLR_EL1.is_set(SCTLR_EL1::C)
        || SCTLR_EL1.is_set(SCTLR_EL1::I)
    {
        crate::utils::halt();
    }
    // Linker guarantees 8-byte-aligned BSS, excluding the active boot stack.
    // Do not touch global Rust state until this clear is complete.
    let mut cursor = addr_of_mut!(sbss).cast::<u64>();
    let end = addr_of_mut!(ebss).cast::<u64>();
    while cursor < end {
        unsafe {
            cursor.write_volatile(0);
            cursor = cursor.add(1);
        }
    }
    boot_main(entry_el)
}

#[derive(Clone, Copy)]
#[repr(C, align(4096))]
struct Table([PageTableEntry; 512]);
impl Table {
    const EMPTY: Self = Self([PageTableEntry::empty(); 512]);
}

static mut ROOT: Table = Table::EMPTY;
static mut RAM_L1: Table = Table::EMPTY;
static mut RAM_L2: Table = Table::EMPTY;
static mut RAM_L3: [Table; 16] = [Table::EMPTY; 16];
static mut DEVICE_L2: Table = Table::EMPTY;
static mut DEVICE_L3: Table = Table::EMPTY;

unsafe extern "C" {
    fn exception_vector_base();
    static boot_stack_top: u8;
    static mut sbss: u8;
    static mut ebss: u8;
    static skernel: u8;
    static etext: u8;
    static erodata: u8;
    static ekernel: u8;
    static stack_guard: u8;
}

// These functions run with the MMU off and interrupts masked. All tables have
// exclusive boot-time ownership; use raw pointers instead of static-mut refs.
unsafe fn set(table: *mut Table, index: usize, entry: PageTableEntry) {
    unsafe {
        addr_of_mut!((*table).0)
            .cast::<PageTableEntry>()
            .add(index)
            .write(entry)
    };
}
fn table_entry(table: *const Table) -> PageTableEntry {
    PageTableEntry::new_table(PhysAddr::from_usize(table as usize))
}

unsafe fn init_page_tables() {
    unsafe {
        set(addr_of_mut!(ROOT), 0, table_entry(addr_of!(RAM_L1)));
        set(addr_of_mut!(RAM_L1), 1, table_entry(addr_of!(RAM_L2)));
        let tables = addr_of_mut!(RAM_L3).cast::<Table>();
        let start = addr_of!(skernel) as usize;
        let end = addr_of!(ekernel) as usize;
        for va in (start..end).step_by(PAGE_SIZE) {
            if va == addr_of!(stack_guard) as usize {
                continue;
            }
            let l2 = (va - RAM_START) >> 21;
            assert!(l2 < 16);
            let l3 = tables.add(l2);
            set(addr_of_mut!(RAM_L2), l2, table_entry(l3));
            let flags = if va < addr_of!(etext) as usize {
                MemFlags::READ | MemFlags::EXECUTE
            } else if va < addr_of!(erodata) as usize {
                MemFlags::READ
            } else {
                MemFlags::READ | MemFlags::WRITE
            };
            set(
                l3,
                (va >> 12) & 511,
                PageTableEntry::new_page(PhysAddr::from_usize(va), flags, false),
            );
        }
        {
            // Keep UART mapped even with LOG=off for the panic console.
            let va = crate::config::PL011_UART_BASE;
            set(addr_of_mut!(RAM_L1), 0, table_entry(addr_of!(DEVICE_L2)));
            set(
                addr_of_mut!(DEVICE_L2),
                (va >> 21) & 511,
                table_entry(addr_of!(DEVICE_L3)),
            );
            set(
                addr_of_mut!(DEVICE_L3),
                (va >> 12) & 511,
                PageTableEntry::new_page(
                    PhysAddr::from_usize(va),
                    MemFlags::READ | MemFlags::WRITE | MemFlags::DEVICE,
                    false,
                ),
            );
        }
    }
}

unsafe fn enable_mmu() {
    MAIR_EL1.write(
        MAIR_EL1::Attr0_Device::nonGathering_nonReordering_EarlyWriteAck
            + MAIR_EL1::Attr1_Normal_Inner::WriteBack_NonTransient_ReadWriteAlloc
            + MAIR_EL1::Attr1_Normal_Outer::WriteBack_NonTransient_ReadWriteAlloc,
    );
    // 48-bit VA, 4 KiB granule, inner-shareable WB/WA, 40-bit PA.
    // Disable TTBR1 walks, avoiding the old high-address identity alias.
    TCR_EL1.set(
        (TCR_EL1::T0SZ.val(16)
            + TCR_EL1::TG0::KiB_4
            + TCR_EL1::SH0::Inner
            + TCR_EL1::IRGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::ORGN0::WriteBack_ReadAlloc_WriteAlloc_Cacheable
            + TCR_EL1::T1SZ.val(16)
            + TCR_EL1::TG1::KiB_4
            + TCR_EL1::EPD1::DisableTTBR1Walks
            + TCR_EL1::IPS::Bits_40)
            .value,
    );
    TTBR0_EL1.set(addr_of!(ROOT) as u64);
    TTBR1_EL1.set(0);
    barrier::dsb(barrier::SY);
    super::instructions::flush_tlb_all();
    // Known platform reset baseline + MMU, D/I caches, stack alignment and WXN.
    SCTLR_EL1.set(SCTLR_RESET);
    SCTLR_EL1.modify(
        SCTLR_EL1::M::Enable
            + SCTLR_EL1::C::Cacheable
            + SCTLR_EL1::I::Cacheable
            + SCTLR_EL1::SA::Enable
            + SCTLR_EL1::SA0::Enable
            + SCTLR_EL1::WXN::Enable,
    );
    barrier::isb(barrier::SY);
}

#[unsafe(no_mangle)]
extern "C" fn boot_main(entry_el: u64) -> ! {
    unsafe {
        crate::BOOT_ENTRY_EL.write_volatile(entry_el);
        init_page_tables();
        enable_mmu();
    }
    crate::rust_main()
}
