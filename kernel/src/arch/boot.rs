//! Fixed, single-core QEMU virt boot with page-granular identity mappings.
//! The low identity layout is temporary; Userspace support must introduce a user TTBR0 layout.
use aarch64_cpu::{asm::barrier, registers::*};
use core::arch::global_asm;
use core::ptr::{addr_of, addr_of_mut};
use memory_addr::PhysAddr;

use super::PageTableEntry;
use crate::config::{MemFlags, PAGE_SIZE, RAM_START};

global_asm!(include_str!("boot.S"));

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
    MAIR_EL1.set(0xff04); // Attr0 Device-nGnRE; Attr1 normal WB/WA.
    // 48-bit VA, 4 KiB granule, inner-shareable WB/WA, 40-bit PA.
    // Disable TTBR1 walks, avoiding the old high-address identity alias.
    TCR_EL1.set(
        16 | (1 << 8) | (1 << 10) | (3 << 12) | (16 << 16) | (1 << 23) | (2 << 30) | (2 << 32),
    );
    TTBR0_EL1.set(addr_of!(ROOT) as u64);
    TTBR1_EL1.set(0);
    barrier::dsb(barrier::SY);
    super::instructions::flush_tlb_all();
    // Known platform reset baseline + MMU, D/I caches, stack alignment and WXN.
    SCTLR_EL1.set(0x30d0_0800 | 1 | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 12) | (1 << 19));
    barrier::isb(barrier::SY);
}

#[unsafe(no_mangle)]
extern "C" fn boot_main(entry_el: u64) -> ! {
    unsafe {
        crate::BOOT_ENTRY_EL.write_volatile(entry_el);
        crate::set_boot_state(1);
        init_page_tables();
        enable_mmu();
        crate::set_boot_state(2);
    }
    crate::rust_main()
}
